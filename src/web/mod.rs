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

#[path = "../../web/src/cover.rs"]
mod cover;
#[path = "../../web/src/render.rs"]
mod render;

use anyhow::Result;
use axum::{
    extract::{Path as AxPath, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Json, Response},
    routing::get,
    Router,
};
use cover::{Element, Elements, Repo};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// Preview rasterization DPI for `pdftoppm` (legible spread, modest file size).
const PREVIEW_DPI: u32 = 110;

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

struct AppState {
    repo: Repo,
    /// The main-crate repo (config model) — used by the interior previewer to
    /// resolve trim/bleed/margin geometry exactly as `build`/`validate` do.
    disco: crate::discover::Repo,
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

async fn serve(repo_root: PathBuf, port: u16, pages: u32) -> Result<()> {
    let repo = Repo::open(&repo_root)?;
    let disco = crate::discover::Repo::find(&repo_root)?;
    println!("bookmill web: repo = {}", repo.root.display());
    println!("bookmill web: books_dir = {}", repo.books_dir.display());

    let state: Shared = Arc::new(AppState { repo, disco, default_pages: pages });
    let app = Router::new()
        .route("/", get(home))
        .route("/index.html", get(home))
        .route("/home.js", get(home_js))
        .route("/book.html", get(book_html))
        .route("/book.js", get(book_js))
        .route("/cover.html", get(cover_html))
        .route("/app.js", get(app_js))
        .route("/preview.html", get(preview_html))
        .route("/preview.js", get(preview_js))
        .route("/api/books", get(api_books))
        .route("/api/book/{slug}", get(api_book))
        .route("/api/cover/{book}/{lang}", get(api_cover).post(api_save))
        .route("/api/asset/{book}/{lang}/{kind}", get(api_asset))
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

async fn api_books(State(st): State<Shared>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let books = st.repo.books().map_err(err)?;
    let list: Vec<_> = books
        .iter()
        .map(|b| {
            serde_json::json!({
                "slug": b.slug,
                "languages": b.languages,
                "titles": b.titles,
                "protected": cover::is_protected(&b.slug),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "repoRoot": st.repo.root.display().to_string(),
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
    let (cfg, _dir) = st.disco.find_book(&slug).map_err(err)?;
    let author = cfg.meta.author.clone().unwrap_or_else(|| st.repo.author.clone());

    let mut langs = serde_json::Map::new();
    for lang in &cfg.languages {
        let pdf = kdp_pdf_path(&st.disco.root, &slug, lang);
        let pages = if pdf.exists() { pdf_pages(&pdf) } else { None };
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
        "langs": langs,
    })))
}

async fn api_cover(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let b = st.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    let resp = cover::load_cover(&st.repo, &b, &lang).map_err(err)?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct SaveBody {
    title: Element,
    subtitle: Element,
    author: Element,
    #[serde(default)]
    bgcolor: String,
}

async fn api_save(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
    Json(body): Json<SaveBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let b = st.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    let els = Elements { title: body.title, subtitle: body.subtitle, author: body.author };
    let bgcolor = if body.bgcolor.trim().is_empty() { "#000000".to_string() } else { body.bgcolor };

    let path = cover::save_cover(&b, &lang, &els, &bgcolor).map_err(err)?;

    // Re-render authoritatively. Pass --pages only if the print interior is absent.
    let kdp_pdf = st
        .repo
        .root
        .join("output")
        .join(&b.slug)
        .join(&lang)
        .join(format!("{}-{}-kdp.pdf", b.slug, lang));
    let pages = (!kdp_pdf.exists()).then_some(st.default_pages);

    let render = render::render_cover(&st.repo.root, &b.slug, &lang, pages);
    let (ok, log) = match render {
        Ok(log) => (true, log),
        Err(e) => (false, format!("{e:#}")),
    };
    let rendered = cover::best_rendered_path(&st.repo, &b, &lang)
        .map(|_| format!("/api/asset/{}/{}/rendered?t={}", b.slug, lang, now_ms()));

    Ok(Json(serde_json::json!({
        "ok": ok,
        "saved": path.display().to_string(),
        "renderLog": log,
        "renderedUrl": rendered,
    })))
}

async fn api_asset(
    State(st): State<Shared>,
    AxPath((book, lang, kind)): AxPath<(String, String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let b = st.repo.find_book(&book).map_err(err)?;
    check_lang(&b.languages, &lang)?;
    safe_segment("kind", &kind)?;
    let path = match kind.as_str() {
        "bg" => cover::bg_path(&st.repo, &b, &lang),
        "rendered" => cover::best_rendered_path(&st.repo, &b, &lang),
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
    let (cfg, _dir) = st.disco.find_book(&book).map_err(err)?;
    check_lang(&cfg.languages, &lang)?;
    let pdf = kdp_pdf_path(&st.disco.root, &book, &lang);
    let exists = pdf.exists();
    let pages = if exists { pdf_pages(&pdf) } else { None };
    let geometry = crate::build::kdp_print_geometry(&st.disco, &cfg).map(|g| {
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
    let _ = st.disco.find_book(&book).map_err(err)?;
    let pdf = kdp_pdf_path(&st.disco.root, &book, &lang);
    if !pdf.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no KDP PDF for {book}/{lang} (build it first): {}", pdf.display()),
        ));
    }
    let png = render_preview_page(&st.disco.root, &book, &lang, &pdf, n).map_err(err)?;
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
    let _ = st.disco.find_book(&book).map_err(err)?;
    let bin = render::bookmill_bin();
    let out = Command::new(&bin)
        .arg("--repo")
        .arg(&st.disco.root)
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
