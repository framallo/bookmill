//! bookmill-web — standalone v1 web UI: cover editor + previewer stub.
//!
//! An axum server that serves a small vanilla-JS + Konva.js frontend and a REST
//! API over a books repo. The editor loads a book's cover (bg image + draggable
//! title/subtitle/author text), saves the layout back into the book's
//! `bookmill.toml`, and triggers an authoritative re-render via `bookmill
//! covers`. The main bookmill crate is NOT modified.
//!
//! Usage: `cargo run -- [--repo <path>] [--port <n>]`  (see web/README.md).

mod cover;
mod render;

use axum::{
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use cover::{Element, Elements, Repo};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

const DEFAULT_REPO: &str = "/Users/framallo/work/argentina_animal_libertaria";

struct AppState {
    repo: Repo,
    /// spine page count used when a book's print interior PDF is absent
    default_pages: u32,
}

type Shared = Arc<AppState>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = parse_args();
    let repo = Repo::open(&cfg.repo)?;
    println!("bookmill-web: repo = {}", repo.root.display());
    println!("bookmill-web: books_dir = {}", repo.books_dir.display());
    println!("bookmill-web: bookmill binary = {}", render::bookmill_bin());

    let state: Shared = Arc::new(AppState { repo, default_pages: cfg.pages });

    let static_dir = static_dir();
    let app = Router::new()
        // Home is the books list (index.html was renamed to cover.html in the
        // three-level nav). The folded `bookmill web` binary is the full UI; this
        // standalone crate serves the static pages + the cover-editor API only
        // (it lacks /api/book, /api/preview, /api/warnings — use `bookmill web`).
        .route("/", get(|| async { axum::response::Redirect::to("/home.html") }))
        .route("/api/books", get(api_books))
        .route("/api/cover/{book}/{lang}", get(api_cover).post(api_save))
        .route("/api/asset/{book}/{lang}/{kind}", get(api_asset))
        .fallback_service(ServeDir::new(&static_dir).append_index_html_on_directories(true))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("\n  Cover editor:  http://{addr}/");
    println!("  Previewer:     http://{addr}/preview.html\n");
    axum::serve(listener, app).await?;
    Ok(())
}

struct Cfg {
    repo: PathBuf,
    port: u16,
    pages: u32,
}

fn parse_args() -> Cfg {
    let mut repo = std::env::var("BOOKMILL_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_REPO));
    let mut port: u16 = std::env::var("BOOKMILL_WEB_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(7777);
    let mut pages: u32 = std::env::var("BOOKMILL_WEB_PAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repo" => {
                if let Some(v) = it.next() {
                    repo = PathBuf::from(v);
                }
            }
            "--port" => {
                if let Some(v) = it.next() {
                    port = v.parse().unwrap_or(port);
                }
            }
            "--pages" => {
                if let Some(v) = it.next() {
                    pages = v.parse().unwrap_or(pages);
                }
            }
            _ => {}
        }
    }
    Cfg { repo, port, pages }
}

fn static_dir() -> PathBuf {
    // Resolve relative to the crate so `cargo run` from anywhere works.
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    if here.exists() {
        here
    } else {
        PathBuf::from("static")
    }
}

// --------------------------------------------------------------------------
// handlers
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
    // strip any query suffix from kind (handled by axum already) — kind is clean.
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
    Ok(([(axum::http::header::CONTENT_TYPE, ct), (axum::http::header::CACHE_CONTROL, "no-cache")], bytes))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
