// Prevent a second console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! bookmill desktop — a Tauri v2 shell around the existing `bookmill web` server.
//!
//! On startup it:
//!   1. resolves the `bookmill` binary (env override → next to this exe → app
//!      bundle Resources → PATH),
//!   2. resolves the content repo (`--repo`/`BOOKMILL_REPO` → walk up from cwd →
//!      native folder picker),
//!   3. spawns `bookmill web --port <ephemeral> --repo <root>`,
//!   4. waits for the port to accept connections, then navigates the native
//!      window at `http://127.0.0.1:<port>/`.
//!
//! Everything the window shows (library grid, cover editor, previewer) is the
//! unmodified axum web UI — no route is reimplemented here.

use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent, Url};
use tauri_plugin_dialog::DialogExt;

fn main() {
    let repo = resolve_repo_from_args();

    let child_slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let run_slot = child_slot.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let slot = child_slot.clone();
            let repo = repo.clone();

            // Do the (blocking) discovery + server spawn off the UI thread so the
            // window paints its splash immediately.
            std::thread::spawn(move || {
                match start_server(&handle, repo) {
                    Ok((port, child)) => {
                        *slot.lock().unwrap() = Some(child);
                        if let Some(win) = handle.get_webview_window("main") {
                            let url = format!("http://127.0.0.1:{port}/");
                            if let Ok(u) = url.parse::<Url>() {
                                let _ = win.navigate(u);
                            }
                        }
                    }
                    Err(e) => splash_error(&handle, &format!("{e:#}")),
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building bookmill desktop");

    app.run(move |_app, event| {
        if let RunEvent::Exit = event {
            if let Some(mut child) = run_slot.lock().unwrap().take() {
                let _ = child.kill();
            }
        }
    });
}

/// Spawn `bookmill web` and wait until it accepts connections. Returns the port.
fn start_server(app: &tauri::AppHandle, repo: Option<PathBuf>) -> anyhow::Result<(u16, Child)> {
    let bin = resolve_bookmill_bin(app)
        .ok_or_else(|| anyhow::anyhow!(
            "could not find the `bookmill` binary. Set BOOKMILL_BIN, put it on PATH, \
             or bundle it into the app (see docs/DESKTOP.md)."
        ))?;

    // A repo is required for discovery. Fall back to a native folder picker.
    let root = match repo.or_else(|| find_repo_root(&std::env::current_dir().ok()?)) {
        Some(r) => r,
        None => pick_repo(app).ok_or_else(|| {
            anyhow::anyhow!("no bookmill repo selected (a folder with bookmill.toml)")
        })?,
    };

    let port = free_port()?;
    let mut cmd = Command::new(&bin);
    cmd.arg("--repo").arg(&root).arg("web").arg("--port").arg(port.to_string());
    cmd.current_dir(&root);
    // A GUI app launched from Finder inherits a minimal PATH
    // (`/usr/bin:/bin:/usr/sbin:/sbin`) that excludes Homebrew, so child tools the
    // server shells out to (notably `pdftoppm` for the interior previewer) can't
    // be found. Prepend the common Homebrew/MacPorts bin dirs so they resolve.
    cmd.env("PATH", augmented_path());
    let child = cmd.spawn().map_err(|e| anyhow::anyhow!("spawning {bin:?}: {e}"))?;

    wait_for_port(port, Duration::from_secs(20))?;
    Ok((port, child))
}

/// Locate the `bookmill` CLI. Order: env override, next to this exe, the app
/// bundle's Resources dir, then PATH.
fn resolve_bookmill_bin(app: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BOOKMILL_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("bookmill");
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    // Bundled as a Tauri resource (see tauri.conf.json `bundle.resources`).
    if let Ok(res) = app.path().resource_dir() {
        let cand = res.join("bookmill");
        if cand.exists() {
            return Some(cand);
        }
    }
    which_on_path("bookmill")
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// `--repo <dir>` on the desktop app's own argv, or `BOOKMILL_REPO`.
fn resolve_repo_from_args() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--repo" {
            if let Some(v) = args.next() {
                return Some(PathBuf::from(v));
            }
        } else if let Some(v) = a.strip_prefix("--repo=") {
            return Some(PathBuf::from(v));
        }
    }
    std::env::var_os("BOOKMILL_REPO").map(PathBuf::from)
}

/// Walk up from `start` looking for a repo-level `bookmill.toml`.
fn find_repo_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut cur = Some(start.to_path_buf());
    while let Some(dir) = cur {
        if dir.join("bookmill.toml").is_file() {
            return Some(dir);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Native folder picker; keep prompting until a bookmill repo is chosen or the
/// user cancels.
fn pick_repo(app: &tauri::AppHandle) -> Option<PathBuf> {
    loop {
        let picked = app.dialog().file().blocking_pick_folder();
        let dir = match picked {
            Some(p) => p.into_path().ok()?,
            None => return None,
        };
        if let Some(root) = find_repo_root(&dir) {
            return Some(root);
        }
        // Not a bookmill repo — let the picker reopen.
    }
}

/// Build a PATH for the spawned `bookmill web` that includes the common Homebrew
/// / MacPorts bin dirs on top of the inherited PATH, so child tools like
/// `pdftoppm` resolve even when the app was launched from Finder (minimal PATH).
fn augmented_path() -> std::ffi::OsString {
    let extra = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"];
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = extra.iter().map(PathBuf::from).collect();
    dirs.extend(std::env::split_paths(&current));
    // Dedup while preserving order (avoid an unboundedly growing PATH).
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    std::env::join_paths(dirs).unwrap_or(current)
}

/// Grab a free localhost port by binding to :0 and releasing it.
fn free_port() -> anyhow::Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// Poll until `port` accepts a TCP connection (i.e. the server is up).
fn wait_for_port(port: u16, timeout: Duration) -> anyhow::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    anyhow::bail!("bookmill web did not come up on port {port} within {timeout:?}")
}

/// Replace the splash body with an error message (best-effort JS eval).
fn splash_error(app: &tauri::AppHandle, msg: &str) {
    if let Some(win) = app.get_webview_window("main") {
        let safe = msg.replace('\\', "\\\\").replace('`', "\\`");
        let js = format!(
            "document.body.innerHTML = \
             '<div style=\"padding:40px;font:14px system-ui;color:#e8807f\">\
             <h2 style=\\'color:#e7edf3\\'>Could not start bookmill</h2>\
             <pre style=\\'white-space:pre-wrap\\'>' + `{safe}` + '</pre></div>';"
        );
        let _ = win.eval(&js);
    }
}
