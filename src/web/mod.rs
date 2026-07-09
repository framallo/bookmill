//! `bookmill web` — the cover editor + previewer, folded into the main binary.
//!
//! An axum server (started on demand by the `web` subcommand) that serves a small
//! vanilla-JS + Konva.js frontend and a REST API over the books repo. The editor
//! loads a book's cover (bg image + draggable title/subtitle/author), saves the
//! layout into the book's `bookmill.toml`, and triggers an authoritative re-render
//! via `bookmill build cover`.
//!
//! The cover/IO logic is **shared by source** with the standalone `web/` crate via
//! `#[path]` (one file, no drift). Static assets are embedded, so this needs no
//! files on disk — a single self-contained binary. The rest of bookmill stays
//! synchronous; only this command spins up a Tokio runtime.
//!
//! The active books repo is **switchable at runtime** (VS Code-style Open Folder /
//! Open Recent): `AppState.active` holds the current `Repo` + `discover::Repo`
//! behind an `RwLock`, and `POST /api/open` swaps it. A recent-folders list is
//! persisted to `~/.config/bookmill/recent.json`.

#[path = "../../web/src/cover.rs"]
mod cover;
#[path = "../../web/src/render.rs"]
mod render;

use anyhow::Result;
use axum::{
    extract::{Path as AxPath, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use cover::{Element, Elements, Repo, WrapElements};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};

/// Preview rasterization DPI for `pdftoppm` (legible spread, modest file size).
const PREVIEW_DPI: u32 = 110;
/// How many recent-folder entries to keep in `recent.json`.
const RECENT_CAP: usize = 15;
/// Audiobook square-cover side (px) derived by cropping the front cover.
const AUDIOBOOK_SIDE: u32 = 3000;

// Embedded frontend (single-binary; no static dir needed). Three-level nav:
// home (books list) → book detail → cover editor / interior previewer.
const HOME_HTML: &str = include_str!("../../web/static/home.html");
const HOME_JS: &str = include_str!("../../web/static/home.js");
const BOOK_HTML: &str = include_str!("../../web/static/book.html");
const BOOK_JS: &str = include_str!("../../web/static/book.js");
const COVER_HTML: &str = include_str!("../../web/static/cover.html");
const APP_JS: &str = include_str!("../../web/static/app.js");
const PREVIEW_HTML: &str = include_str!("../../web/static/preview.html");
const PREVIEW_JS: &str = include_str!("../../web/static/preview.js");
const APP_CSS: &str = include_str!("../../web/static/app.css");
const A11Y_JS: &str = include_str!("../../web/static/a11y.js");

/// The currently-open books repo, in both models the server needs:
///   * `repo` — the cover/IO model (`toml_edit`-based, used by the cover editor).
///   * `disco` — the main-crate config model (used by the previewer/geometry).
/// Held behind an `RwLock` in `AppState` so `POST /api/open` can swap the active
/// folder without restarting the server.
struct Active {
    root: PathBuf,
    repo: Repo,
    /// The main-crate repo (config model) — used by the interior previewer to
    /// resolve trim/bleed/margin geometry exactly as `build`/`validate` do.
    disco: crate::discover::Repo,
}

struct AppState {
    /// The active project (switchable at runtime — see `POST /api/open`).
    active: RwLock<Active>,
    /// spine page count used when a book's print interior PDF is absent
    default_pages: u32,
}

type Shared = Arc<AppState>;

/// Start the cover-editor web server (blocking). Builds a Tokio runtime so the
/// rest of the CLI can stay synchronous.
pub fn run(repo_root: &Path, port: u16, pages: u32) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(serve(repo_root.to_path_buf(), port, pages))
}

/// Open (or re-open) a books repo folder, deriving both config models.
fn open_active(root: &Path) -> Result<Active> {
    let repo = Repo::open(root)?;
    let disco = crate::discover::Repo::find(root)?;
    Ok(Active { root: root.to_path_buf(), repo, disco })
}

async fn serve(repo_root: PathBuf, port: u16, pages: u32) -> Result<()> {
    let active = open_active(&repo_root)?;
    println!("bookmill web: repo = {}", active.repo.root.display());
    println!("bookmill web: books_dir = {}", active.repo.books_dir.display());
    // Seed the recents list with the folder we booted on so Open Recent shows it.
    push_recent(&active.root);

    let state: Shared = Arc::new(AppState {
        active: RwLock::new(active),
        default_pages: pages,
    });
    let app = Router::new()
        .route("/", get(home))
        .route("/index.html", get(home))
        .route("/home.js", get(home_js))
        .route("/book.html", get(book_html))
        .route("/book.js", get(book_js))
        .route("/cover.html", get(cover_html))
        .route("/app.js", get(app_js))
        .route("/app.css", get(app_css))
        .route("/a11y.js", get(a11y_js))
        .route("/preview.html", get(preview_html))
        .route("/preview.js", get(preview_js))
        // projects (Open Folder / Open Recent)
        .route("/api/projects", get(api_projects))
        .route("/api/open", post(api_open))
        // books + book detail
        .route("/api/books", get(api_books))
        .route("/api/book/{slug}", get(api_book))
        // outputs (build artifacts): list, serve one, (re)build one
        .route("/api/outputs/{book}/{lang}", get(api_outputs))
        .route("/api/output/{book}/{lang}/{kind}", get(api_output))
        .route("/api/build/{book}/{lang}", post(api_build))
        // cover editor
        .route("/api/cover/{book}/{lang}", get(api_cover).post(api_save))
        .route("/api/cover/{book}/{lang}/svg", get(api_cover_svg))
        .route("/api/cover/{book}/{lang}/audiobook", post(api_audiobook_cover))
        .route("/api/asset/{book}/{lang}/{kind}", get(api_asset))
        // interior previewer
        .route("/api/preview/{book}/{lang}", get(api_preview_meta))
        .route("/api/preview/{book}/{lang}/page/{n}", get(api_preview_page))
        .route("/api/warnings/{book}/{lang}", get(api_warnings))
        // DNS-rebinding guard: the server binds to 127.0.0.1 and its routes write
        // `bookmill.toml` and spawn processes, so reject any request whose Host
        // header isn't a loopback name (H2). The desktop WKWebView and a local
        // browser both send `127.0.0.1`/`localhost`; a rebinding attacker page
        // would carry its own hostname and is rejected. No CORS layer: the SPA is
        // same-origin and needs none (dropping the former permissive `*`).
        .layer(middleware::from_fn(guard_host))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("\n  Home (books):  http://{addr}/");
    println!("  Cover editor:  http://{addr}/cover.html?book=<slug>&lang=<lang>");
    println!("  Previewer:     http://{addr}/preview.html?book=<slug>&lang=<lang>\n");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Loopback-only Host guard (anti-DNS-rebinding). Accepts requests whose Host
/// header names a loopback host (`localhost`, `127.0.0.1`, `::1`) on any port,
/// and requests with no Host header (HTTP/1.0 / same-process probes). Rejects
/// everything else with 403 so a rebinding page pointing a public hostname at
/// 127.0.0.1 cannot drive the config-writing / process-spawning API.
async fn guard_host(req: Request, next: Next) -> Response {
    match req.headers().get(header::HOST) {
        None => next.run(req).await,
        Some(h) => {
            let host = h.to_str().unwrap_or("").trim();
            let hostname = if let Some(rest) = host.strip_prefix('[') {
                rest.split(']').next().unwrap_or("") // [::1]:port
            } else {
                host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
            };
            if matches!(hostname, "localhost" | "127.0.0.1" | "::1") {
                next.run(req).await
            } else {
                (StatusCode::FORBIDDEN, "forbidden host").into_response()
            }
        }
    }
}

// --------------------------------------------------------------------------
// static handlers (embedded)
// --------------------------------------------------------------------------

fn js(body: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], body)
}

async fn home() -> impl IntoResponse {
    Html(HOME_HTML)
}
async fn home_js() -> impl IntoResponse {
    js(HOME_JS)
}
async fn book_html() -> impl IntoResponse {
    Html(BOOK_HTML)
}
async fn book_js() -> impl IntoResponse {
    js(BOOK_JS)
}
async fn cover_html() -> impl IntoResponse {
    Html(COVER_HTML)
}
async fn preview_html() -> impl IntoResponse {
    Html(PREVIEW_HTML)
}
async fn app_js() -> impl IntoResponse {
    js(APP_JS)
}
async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], APP_CSS)
}
async fn a11y_js() -> impl IntoResponse {
    js(A11Y_JS)
}
async fn preview_js() -> impl IntoResponse {
    js(PREVIEW_JS)
}

// --------------------------------------------------------------------------
// API handlers
// --------------------------------------------------------------------------

fn err(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, format!("{e:#}"))
}

/// Reject URL path segments that could escape the books tree before they are used
/// in a filesystem join (M3). `book` is already validated via `find_book` (must be
/// a known slug); `lang`/`kind` were passed straight through. Confine them to
/// `[A-Za-z0-9._-]` with no `..` and no path separators.
fn safe_segment(kind: &str, s: &str) -> Result<(), (StatusCode, String)> {
    let ok = !s.is_empty()
        && s != ".."
        && !s.contains("..")
        && !s.contains('/')
        && !s.contains('\\')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err((StatusCode::BAD_REQUEST, format!("invalid {kind} segment: {s:?}")))
    }
}

/// Validate `lang` against a book's declared languages (a stronger check than the
/// character allowlist: it must be a language the book actually declares).
fn check_lang(declared: &[String], lang: &str) -> Result<(), (StatusCode, String)> {
    safe_segment("lang", lang)?;
    if declared.iter().any(|l| l == lang) {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            format!("unknown lang {lang:?} (declared: {})", declared.join(", ")),
        ))
    }
}

// --------------------------------------------------------------------------
// Projects: Open Folder / Open Recent (runtime-switchable active repo)
// --------------------------------------------------------------------------

/// `~/.config/bookmill/recent.json` — the recent-folders list (absolute paths,
/// most-recent-first). None only if `$HOME` is unset (never on a normal Mac).
fn recent_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("bookmill").join("recent.json"))
}

/// Read the recent-folders list (best-effort; a missing/garbage file → empty).
fn load_recents() -> Vec<String> {
    let Some(p) = recent_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    serde_json::from_str::<Vec<String>>(&text).unwrap_or_default()
}

/// Persist the recent-folders list (best-effort; failures are non-fatal).
fn save_recents(list: &[String]) {
    let Some(p) = recent_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(list) {
        let _ = std::fs::write(&p, text);
    }
}

/// Prepend `path` to the recents list (dedup, most-recent-first, capped).
fn push_recent(path: &Path) {
    let p = path.display().to_string();
    let mut list = load_recents();
    list.retain(|x| x != &p);
    list.insert(0, p);
    list.truncate(RECENT_CAP);
    save_recents(&list);
}

/// A folder is a valid project if `<path>/bookmill.toml` parses AND is a *repo*
/// config (has no top-level `slug` — that would make it a book config).
fn validate_repo_folder(path: &Path) -> Result<(), String> {
    let cfg = path.join("bookmill.toml");
    let text = std::fs::read_to_string(&cfg)
        .map_err(|e| format!("no readable bookmill.toml in {}: {e}", path.display()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("bookmill.toml is not valid TOML: {e}"))?;
    if table.contains_key("slug") {
        return Err(format!(
            "{} looks like a BOOK config (has `slug`); open the repo root instead",
            path.display()
        ));
    }
    Ok(())
}

/// `GET /api/projects` — the active project path + the recent-folders list.
async fn api_projects(State(st): State<Shared>) -> impl IntoResponse {
    let active = st.active.read().unwrap().root.display().to_string();
    Json(serde_json::json!({
        "active": active,
        "recent": load_recents(),
    }))
}

#[derive(Deserialize)]
struct OpenBody {
    path: String,
}

/// `POST /api/open` — switch the active books repo to `{ path }`. Validates that
/// the path is absolute, exists, and holds a repo (not book) `bookmill.toml`;
/// on success swaps `AppState.active` and prepends the folder to recents. A bad
/// path returns a clear error and never corrupts the running state.
async fn api_open(
    State(st): State<Shared>,
    Json(body): Json<OpenBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let raw = body.path.trim();
    if raw.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty path".into()));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err((StatusCode::BAD_REQUEST, format!("path must be absolute: {raw}")));
    }
    // Canonicalize (resolves symlinks + normalizes `.`/`..`); rejects nonexistent.
    let path = path
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("cannot open {raw}: {e}")))?;
    if !path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("not a folder: {}", path.display())));
    }
    validate_repo_folder(&path).map_err(|m| (StatusCode::BAD_REQUEST, m))?;

    // Build the new models BEFORE taking the write lock so a failure leaves the
    // current project untouched.
    let next = open_active(&path).map_err(err)?;
    {
        let mut act = st.active.write().unwrap();
        *act = next;
    }
    push_recent(&path);
    Ok(Json(serde_json::json!({
        "ok": true,
        "active": path.display().to_string(),
    })))
}

async fn api_books(State(st): State<Shared>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let books = act.repo.books().map_err(err)?;
    let list: Vec<_> = books
        .iter()
        .map(|b| {
            // Publishing-status ribbon (live/in-review/blocked/draft) from the book's
            // [status] table — read through the config model.
            let status = act
                .disco
                .find_book(&b.slug)
                .ok()
                .map(|(c, _)| c.status.ribbon())
                .unwrap_or("draft");
            serde_json::json!({
                "slug": b.slug,
                "languages": b.languages,
                "titles": b.titles,
                "protected": cover::is_protected(&b.slug),
                "status": status,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "repoRoot": act.repo.root.display().to_string(),
        "books": list,
    })))
}

/// `GET /api/book/{slug}` — config-derived attributes for the book detail page:
/// languages, editions, author/series, and a per-language summary (title/subtitle,
/// built page count, KDP listing facts). Read from the book's `bookmill.toml` via
/// the main-crate config model (`crate::discover` / `crate::config`).
async fn api_book(
    State(st): State<Shared>,
    AxPath(slug): AxPath<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let (cfg, _dir) = act.disco.find_book(&slug).map_err(err)?;
    let author = cfg.meta.author.clone().unwrap_or_else(|| act.repo.author.clone());
    // Repo-level Book (for cover asset lookup); may be absent if discovery differs.
    let book = act.repo.find_book(&slug).ok();

    let mut langs = serde_json::Map::new();
    for lang in &cfg.languages {
        let pdf = kdp_pdf_path(&act.disco.root, &slug, lang);
        let pages = if pdf.exists() { pdf_pages(&pdf) } else { None };
        let cover_exists = book
            .as_ref()
            .map(|b| cover::best_rendered_path(&act.repo, b, lang).is_some())
            .unwrap_or(false);
        let listing = cfg.listing.get(lang).map(|l| {
            serde_json::json!({
                "keywords": l.keywords,
                "bisac": l.bisac,
                "readingAge": l.reading_age,
                "blurbChars": l.blurb.as_ref().map(|b| b.chars().count()),
            })
        });
        langs.insert(
            lang.clone(),
            serde_json::json!({
                "title": cfg.title.get(lang),
                "subtitle": cfg.subtitle.get(lang),
                "kdpPdfExists": pdf.exists(),
                "coverExists": cover_exists,
                "pages": pages,
                "listing": listing,
            }),
        );
    }

    Ok(Json(serde_json::json!({
        "slug": cfg.slug,
        "languages": cfg.languages,
        "editions": cfg.editions,
        "author": author,
        "series": cfg.meta.series,
        "protected": cover::is_protected(&slug),
        "status": cfg.status.ribbon(),
        "langs": langs,
    })))
}

// --------------------------------------------------------------------------
// Outputs: concrete build artifacts (list / serve / (re)build)
// --------------------------------------------------------------------------

/// One buildable output flavor for a book × language. `kind` is the stable id
/// used in URLs and the build API; `file` is the artifact name inside
/// `output/<slug>/<lang>/`; `build` is the `bookmill` build token that produces it.
struct OutputSpec {
    kind: &'static str,
    label: &'static str,
    file: String,
    build: &'static str,
}

/// The canonical output set for a book/lang. Mirrors bookmill's filename scheme
/// (`build.rs::job_output_path`) plus the cover/audiobook artifacts. Order is the
/// display order in the book detail "Outputs" section.
fn output_specs(slug: &str, lang: &str) -> Vec<OutputSpec> {
    let base = format!("{slug}-{lang}");
    vec![
        OutputSpec { kind: "retail-epub", label: "Retail EPUB", file: format!("{base}.epub"), build: "epub" },
        OutputSpec { kind: "kdp-epub", label: "KDP EPUB", file: format!("{base}-kdp.epub"), build: "kdp" },
        OutputSpec { kind: "retail-pdf", label: "Retail PDF", file: format!("{base}.pdf"), build: "pdf" },
        OutputSpec { kind: "kdp-pdf", label: "KDP print PDF", file: format!("{base}-kdp.pdf"), build: "print" },
        OutputSpec { kind: "wrap-cover", label: "Paperback wrap cover", file: format!("{base}-cover-wrap-kdp.pdf"), build: "cover" },
        OutputSpec { kind: "audiobook", label: "Audiobook (m4b)", file: format!("{base}.m4b"), build: "audiobook" },
    ]
}

/// Absolute path of an output artifact inside `output/<slug>/<lang>/`.
fn output_path(root: &Path, slug: &str, lang: &str, file: &str) -> PathBuf {
    root.join("output").join(slug).join(lang).join(file)
}

/// `GET /api/outputs/{book}/{lang}` — the output rows for the detail page: for
/// each flavor, whether it is built and its last-built time (ms since epoch).
async fn api_outputs(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let b = act.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    let rows: Vec<_> = output_specs(&book, &lang)
        .into_iter()
        .map(|s| {
            let p = output_path(&act.repo.root, &book, &lang, &s.file);
            let (exists, mtime) = match std::fs::metadata(&p) {
                Ok(m) => (
                    true,
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis()),
                ),
                Err(_) => (false, None),
            };
            serde_json::json!({
                "kind": s.kind,
                "label": s.label,
                "file": s.file,
                "exists": exists,
                "mtime": mtime,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "slug": book, "lang": lang, "outputs": rows })))
}

/// `GET /api/output/{book}/{lang}/{kind}` — serve a built artifact inline (PDFs /
/// EPUBs open in the browser; the m4b downloads). 404 when not yet built.
async fn api_output(
    State(st): State<Shared>,
    AxPath((book, lang, kind)): AxPath<(String, String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let b = act.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    safe_segment("kind", &kind)?;
    let spec = output_specs(&book, &lang)
        .into_iter()
        .find(|s| s.kind == kind)
        .ok_or((StatusCode::BAD_REQUEST, format!("unknown output kind: {kind}")))?;
    let path = output_path(&act.repo.root, &book, &lang, &spec.file);
    let bytes = std::fs::read(&path).map_err(|_| {
        (StatusCode::NOT_FOUND, format!("not built yet: {}", spec.file))
    })?;
    let ct = match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf",
        Some("epub") => "application/epub+zip",
        Some("m4b") => "audio/mp4",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    // Inline so PDFs/EPUBs preview in a tab; the filename is kept for downloads.
    let disp = format!("inline; filename=\"{}\"", spec.file);
    Ok((
        [
            (header::CONTENT_TYPE, ct.to_string()),
            (header::CONTENT_DISPOSITION, disp),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        bytes,
    ))
}

#[derive(Deserialize)]
struct BuildBody {
    /// output kind ("retail-epub" … "audiobook") or "all" for everything.
    kind: String,
}

/// Run one `bookmill` build invocation and collect its combined log.
fn run_build(root: &Path, args: &[&str]) -> (bool, String) {
    let bin = render::bookmill_bin();
    let mut cmd = Command::new(&bin);
    cmd.arg("--repo").arg(root);
    for a in args {
        cmd.arg(a);
    }
    let mut log = format!("$ {bin} --repo {} {}\n", root.display(), args.join(" "));
    match cmd.output() {
        Ok(out) => {
            log.push_str(&String::from_utf8_lossy(&out.stdout));
            log.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), log)
        }
        Err(e) => {
            log.push_str(&format!("failed to spawn {bin}: {e}"));
            (false, log)
        }
    }
}

/// `POST /api/build/{book}/{lang}` — (re)generate one output (or everything with
/// `{ "kind": "all" }`) by shelling to the `bookmill` binary. Returns the combined
/// build log + a success flag. Blocking on purpose (mirrors the cover re-render).
async fn api_build(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
    Json(body): Json<BuildBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Snapshot the root + book validity under the lock, then release it: the build
    // shells out and can take a while, and we must not hold the lock across it.
    let root = {
        let act = st.active.read().unwrap();
        let b = act.repo.find_book(&book).map_err(err)?;
        check_lang(&b.languages, &lang)?;
        act.repo.root.clone()
    };

    let kind = body.kind.trim();
    let (ok, log) = match kind {
        // Everything: all interiors, then the cover set (front PNG + wrap PDF).
        "all" => {
            let (ok1, log1) = run_build(&root, &["build", &book, "--lang", &lang]);
            let (ok2, log2) = run_build(&root, &["build", "cover", &book, "--lang", &lang]);
            (ok1 && ok2, format!("{log1}\n{log2}"))
        }
        // Cover wrap/front.
        "cover" | "wrap-cover" => run_build(&root, &["build", "cover", &book, "--lang", &lang]),
        // Audiobook (kab; personal use).
        "audiobook" => run_build(&root, &["audiobook", &book, "--lang", &lang]),
        // Interior formats: resolve the kind → build token via the spec table.
        _ => {
            let spec = output_specs(&book, &lang).into_iter().find(|s| s.kind == kind);
            match spec {
                Some(s) => run_build(&root, &["build", &book, "--lang", &lang, "--format", s.build]),
                None => return Err((StatusCode::BAD_REQUEST, format!("unknown build kind: {kind}"))),
            }
        }
    };
    Ok(Json(serde_json::json!({ "ok": ok, "log": log })))
}

// --------------------------------------------------------------------------
// Cover editor
// --------------------------------------------------------------------------

async fn api_cover(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let b = act.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    let resp = cover::load_cover(&act.repo, &b, &lang).map_err(err)?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct SvgQuery {
    /// `wrap=1` → the full paperback wrap (back+spine+front); default is the front.
    #[serde(default)]
    wrap: u8,
}

/// `GET /api/cover/{book}/{lang}/svg?wrap=0|1` — the **authoritative** cover SVG
/// the build rasterizes, served straight to the editor so its canvas *is* the
/// real output (no Konva re-implementation, no drift). Text is pre-laid-out
/// server-side with real font metrics and the bg is an embedded data URI, so the
/// browser renders it standalone. `wrap=1` returns the full wrap (spine width from
/// the built KDP interior's page count, else `default_pages`).
async fn api_cover_svg(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
    Query(q): Query<SvgQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let (cfg, dir) = act.disco.find_book(&book).map_err(err)?;
    check_lang(&cfg.languages, &lang)?;
    let wrap = q.wrap != 0;
    let pages = pdf_pages(&kdp_pdf_path(&act.disco.root, &book, &lang)).unwrap_or(st.default_pages);
    let svg = bookmill::editor_cover_svg(&act.disco.root, &dir, &lang, wrap, pages).map_err(err)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        svg,
    ))
}

#[derive(Deserialize)]
struct SaveBody {
    title: Element,
    subtitle: Element,
    author: Element,
    #[serde(default)]
    bgcolor: String,
    /// Optional back-cover blurb (paperback wrap). When present it is written to
    /// `[cover.<lang>].blurb` before the re-render so the wrap PDF reflects it.
    #[serde(default)]
    blurb: Option<String>,
    /// Optional back-panel drag layout (paperback wrap). When present its blocks
    /// are written to `[cover.<lang>.wrap]` before the re-render so the wrap PDF
    /// positions the back-cover blurb/badge/author absolutely.
    #[serde(default)]
    wrap: Option<WrapElements>,
}

async fn api_save(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
    Json(body): Json<SaveBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let b = act.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    let els = Elements { title: body.title, subtitle: body.subtitle, author: body.author };
    let bgcolor = if body.bgcolor.trim().is_empty() { "#000000".to_string() } else { body.bgcolor };

    let path = cover::save_cover(&b, &lang, &els, &bgcolor).map_err(err)?;
    // Back-cover blurb (wrap): persist to `[cover.<lang>].blurb` when supplied.
    if let Some(blurb) = &body.blurb {
        cover::save_blurb(&b, &lang, blurb).map_err(err)?;
    }
    // Back-panel drag layout (wrap): persist to `[cover.<lang>.wrap]` when supplied.
    if let Some(wrap) = &body.wrap {
        cover::save_wrap_layout(&b, &lang, wrap).map_err(err)?;
    }

    // Re-render authoritatively. Pass --pages only if the print interior is absent.
    let kdp_pdf = act
        .repo
        .root
        .join("output")
        .join(&b.slug)
        .join(&lang)
        .join(format!("{}-{}-kdp.pdf", b.slug, lang));
    let pages = (!kdp_pdf.exists()).then_some(st.default_pages);

    let render = render::render_cover(&act.repo.root, &b.slug, &lang, pages);
    let (ok, log) = match render {
        Ok(log) => (true, log),
        Err(e) => (false, format!("{e:#}")),
    };
    let rendered = cover::best_rendered_path(&act.repo, &b, &lang)
        .map(|_| format!("/api/asset/{}/{}/rendered?t={}", b.slug, lang, now_ms()));

    Ok(Json(serde_json::json!({
        "ok": ok,
        "saved": path.display().to_string(),
        "renderLog": log,
        "renderedUrl": rendered,
    })))
}

/// `POST /api/cover/{book}/{lang}/audiobook` — derive the SQUARE audiobook cover
/// from the rendered eBook front by center-cropping it to a square and scaling to
/// 3000×3000. Saved as `libros/<slug>/cover/audiobook-<lang>.png`. This is the
/// "infer the audiobook cover from the wrap/front" flow: the front title block +
/// scene become the square art with no separate design.
async fn api_audiobook_cover(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let b = act.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    let src = cover::best_rendered_path(&act.repo, &b, &lang).ok_or((
        StatusCode::NOT_FOUND,
        "no rendered front cover yet — render the front/wrap cover first".to_string(),
    ))?;
    let dst = cover::audiobook_cover_path(&b, &lang);
    make_square_cover(&src, &dst, AUDIOBOOK_SIDE)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "saved": dst.display().to_string(),
        "url": format!("/api/asset/{}/{}/audiobook?t={}", b.slug, lang, now_ms()),
    })))
}

/// Center-crop `src` to a square and resize to `side`×`side`, writing a PNG to
/// `dst`. Used to infer the audiobook square cover from the portrait front.
fn make_square_cover(src: &Path, dst: &Path, side: u32) -> Result<()> {
    let img = image::open(src)?;
    let (w, h) = (img.width(), img.height());
    let s = w.min(h);
    let x = (w - s) / 2;
    let y = (h - s) / 2;
    let square = image::imageops::crop_imm(&img, x, y, s, s).to_image();
    let out = image::imageops::resize(&square, side, side, image::imageops::FilterType::Lanczos3);
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    out.save(dst)?;
    Ok(())
}

async fn api_asset(
    State(st): State<Shared>,
    AxPath((book, lang, kind)): AxPath<(String, String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let b = act.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    safe_segment("kind", &kind)?;
    let path = match kind.as_str() {
        "bg" => cover::bg_path(&act.repo, &b, &lang),
        "rendered" => cover::best_rendered_path(&act.repo, &b, &lang),
        "audiobook" => {
            let p = cover::audiobook_cover_path(&b, &lang);
            p.exists().then_some(p)
        }
        _ => None,
    };
    let path = path.ok_or((StatusCode::NOT_FOUND, format!("asset not found: {kind}")))?;
    let bytes = std::fs::read(&path).map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    let ct = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };
    Ok((
        [(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "no-cache")],
        bytes,
    ))
}

// --------------------------------------------------------------------------
// Interior previewer (two-page spread + KDP-style warnings)
// --------------------------------------------------------------------------

/// Canonical KDP print interior path for a book/lang.
fn kdp_pdf_path(root: &Path, slug: &str, lang: &str) -> PathBuf {
    root.join("output")
        .join(slug)
        .join(lang)
        .join(format!("{slug}-{lang}-kdp.pdf"))
}

/// Page count of a PDF (native, via `lopdf`). None if missing/unreadable.
fn pdf_pages(pdf: &Path) -> Option<u32> {
    crate::pdfmeta::page_count(pdf)
}

/// `GET /api/preview/{book}/{lang}` — interior metadata: page count, the KDP PDF
/// path/existence, and the trim/bleed/margin geometry (resolved from config the
/// same way `build`/`validate` do).
async fn api_preview_meta(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let act = st.active.read().unwrap();
    let (cfg, _dir) = act.disco.find_book(&book).map_err(err)?;
    check_lang(&cfg.languages, &lang)?;
    let pdf = kdp_pdf_path(&act.disco.root, &book, &lang);
    let exists = pdf.exists();
    let pages = if exists { pdf_pages(&pdf) } else { None };
    let geometry = crate::build::kdp_print_geometry(&act.disco, &cfg).map(|g| {
        serde_json::json!({
            "trimW": g.trim_w,
            "trimH": g.trim_h,
            "bleed": g.bleed,
            "pageW": g.geom.pw,
            "pageH": g.geom.ph,
            "margins": {
                "top": g.geom.top,
                "bottom": g.geom.bottom,
                "inner": g.geom.inner,
                "outer": g.geom.outer,
                "bindingoffset": g.geom.bindingoffset,
            },
        })
    });
    Ok(Json(serde_json::json!({
        "slug": book,
        "lang": lang,
        "pdfExists": exists,
        "pdfPath": pdf.display().to_string(),
        "pages": pages,
        "previewDpi": PREVIEW_DPI,
        "geometry": geometry,
    })))
}

/// Locate the `pdftoppm` (poppler) binary. A macOS GUI app launched from Finder
/// (or the packaged `.app`) gets a minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`)
/// that excludes Homebrew, so a bare `Command::new("pdftoppm")` fails there even
/// though it works from a terminal. Search common absolute install locations
/// first, then fall back to `$PATH`. Returns `None` if truly not installed.
fn find_pdftoppm() -> Option<PathBuf> {
    // Explicit override wins (parity with how the desktop resolves the CLI).
    if let Some(p) = std::env::var_os("PDFTOPPM").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    const COMMON: &[&str] = &[
        "/opt/homebrew/bin/pdftoppm", // Apple-Silicon Homebrew
        "/usr/local/bin/pdftoppm",    // Intel Homebrew / manual installs
        "/opt/local/bin/pdftoppm",    // MacPorts
        "/usr/bin/pdftoppm",          // system / Linux distro packages
    ];
    for c in COMMON {
        let p = Path::new(c);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    // Finally, honor whatever `$PATH` the process actually has.
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("pdftoppm");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Rasterize page `n` of the KDP PDF to a cached PNG via `pdftoppm`, returning
/// the PNG path. Cache key includes the DPI; the cache is invalidated when the
/// PDF is newer than the cached image.
fn render_preview_page(root: &Path, slug: &str, lang: &str, pdf: &Path, n: u32) -> anyhow::Result<PathBuf> {
    let dir = root
        .join("output")
        .join(".preview-cache")
        .join(slug)
        .join(lang);
    std::fs::create_dir_all(&dir)?;
    let prefix = dir.join(format!("p{n}-r{PREVIEW_DPI}"));
    let png = dir.join(format!("p{n}-r{PREVIEW_DPI}.png"));
    let fresh = match (std::fs::metadata(&png), std::fs::metadata(pdf)) {
        (Ok(a), Ok(b)) => match (a.modified(), b.modified()) {
            (Ok(pm), Ok(sm)) => pm >= sm,
            _ => false,
        },
        _ => false,
    };
    if fresh {
        return Ok(png);
    }
    let bin = find_pdftoppm().ok_or_else(|| {
        anyhow::anyhow!(
            "pdftoppm (poppler) not found. Install it with `brew install poppler` \
             (searched /opt/homebrew/bin, /usr/local/bin, /opt/local/bin, /usr/bin, and $PATH)."
        )
    })?;
    // `-singlefile` makes pdftoppm write exactly `<prefix>.png` (no page suffix).
    let out = Command::new(&bin)
        .arg("-png")
        .arg("-singlefile")
        .arg("-r")
        .arg(PREVIEW_DPI.to_string())
        .arg("-f")
        .arg(n.to_string())
        .arg("-l")
        .arg(n.to_string())
        .arg(pdf)
        .arg(&prefix)
        .output()
        .map_err(|e| anyhow::anyhow!("running pdftoppm ({}): {e}", bin.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "pdftoppm failed on page {n}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(png)
}

/// `GET /api/preview/{book}/{lang}/page/{n}` — PNG of interior page `n`.
async fn api_preview_page(
    State(st): State<Shared>,
    AxPath((book, lang, n)): AxPath<(String, String, u32)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (root, _ok) = {
        let act = st.active.read().unwrap();
        let _ = act.disco.find_book(&book).map_err(err)?;
        (act.disco.root.clone(), ())
    };
    let pdf = kdp_pdf_path(&root, &book, &lang);
    if !pdf.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no KDP PDF for {book}/{lang} (build it first): {}", pdf.display()),
        ));
    }
    let png = render_preview_page(&root, &book, &lang, &pdf, n).map_err(err)?;
    let bytes = std::fs::read(&png).map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    ))
}

/// `GET /api/warnings/{book}/{lang}` — KDP-style warnings, sourced by shelling to
/// `bookmill validate <book> --deep --json` (the same exe — see render.rs). Issues
/// are filtered to those for this language (lang-agnostic issues are kept).
async fn api_warnings(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let root = {
        let act = st.active.read().unwrap();
        let _ = act.disco.find_book(&book).map_err(err)?;
        act.disco.root.clone()
    };
    let bin = render::bookmill_bin();
    let out = Command::new(&bin)
        .arg("--repo")
        .arg(&root)
        .arg("validate")
        .arg(&book)
        .arg("--deep")
        .arg("--json")
        .output()
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("running {bin} validate: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!(
                "validate --json parse failed: {e}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    })?;
    let issues = parsed
        .get("issues")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|it| match it.get("lang").and_then(|l| l.as_str()) {
                    Some(l) => l == lang,
                    None => true, // lang-agnostic (book-level config / house rules)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "slug": book,
        "lang": lang,
        "summary": parsed.get("summary").cloned().unwrap_or(serde_json::Value::Null),
        "issues": issues,
    })))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
