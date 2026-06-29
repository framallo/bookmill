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
    extract::{Path as AxPath, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use cover::{Element, Elements, Repo};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

// Embedded frontend (single-binary; no static dir needed).
const INDEX_HTML: &str = include_str!("../../web/static/index.html");
const APP_JS: &str = include_str!("../../web/static/app.js");
const PREVIEW_HTML: &str = include_str!("../../web/static/preview.html");
const PREVIEW_JS: &str = include_str!("../../web/static/preview.js");

struct AppState {
    repo: Repo,
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
    println!("bookmill web: repo = {}", repo.root.display());
    println!("bookmill web: books_dir = {}", repo.books_dir.display());

    let state: Shared = Arc::new(AppState { repo, default_pages: pages });
    let app = Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/app.js", get(app_js))
        .route("/preview.html", get(preview_html))
        .route("/preview.js", get(preview_js))
        .route("/api/books", get(api_books))
        .route("/api/cover/{book}/{lang}", get(api_cover).post(api_save))
        .route("/api/asset/{book}/{lang}/{kind}", get(api_asset))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("\n  Cover editor:  http://{addr}/");
    println!("  Previewer:     http://{addr}/preview.html\n");
    axum::serve(listener, app).await?;
    Ok(())
}

// --------------------------------------------------------------------------
// static handlers (embedded)
// --------------------------------------------------------------------------

async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}
async fn preview_html() -> impl IntoResponse {
    Html(PREVIEW_HTML)
}
async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], APP_JS)
}
async fn preview_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], PREVIEW_JS)
}

// --------------------------------------------------------------------------
// API handlers
// --------------------------------------------------------------------------

fn err(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, format!("{e:#}"))
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

async fn api_cover(
    State(st): State<Shared>,
    AxPath((book, lang)): AxPath<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let b = st.repo.find_book(&book).map_err(err)?;
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

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
