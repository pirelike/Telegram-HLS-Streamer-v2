// ============================================================
// THLS — Apple-inspired UI · DROP-IN frontend.rs
// ------------------------------------------------------------
// Save as: src/api/frontend.rs   (replaces the existing file)
//
// Public API (handler functions + routes) is UNCHANGED.
// What's new in the markup:
//   • Top-tab navigation (Home / Films / Series / Anime Films
//     / Anime TV) replaces the old sidebar.
//   • Glass navbar with brand mark, search, ⌘K palette trigger,
//     upload, theme toggle, settings.
//   • Browse shell exposes #thlsHero + #videosContainer so the
//     new browse.js can render a hero on Home (only) and rows.
//   • Existing inner DOM IDs (#videosContainer, #editModal,
//     drop-zone, settings fields, watch player) are preserved —
//     browse.js / upload.js / settings.js / watch.js need no
//     changes for everything except Home, which uses a tiny
//     new file: static/browse-home.js (also in this folder).
// ============================================================

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use serde_json::{json, Value};

use super::frontend_bodies::{browse_body, settings_body, upload_body, watch_body};
use super::{api_error, db_unavailable, valid_job_id, AppState};
use crate::db;

// ─── handlers (unchanged signatures) ────────────────────────────────
pub(super) async fn handle_home() -> Html<String> {
    browse_page(
        "Home",
        "home",
        json!({
            "category": "all",
            "view": "home",  // ← new: triggers hero + rows in browse.js
            "seriesName": Value::Null,
            "seriesSlug": Value::Null,
            "seasonNumber": Value::Null,
            "breadcrumbs": [],
        }),
    )
}

pub(super) async fn handle_films() -> Html<String> {
    browse_page("Films", "films", category_ctx("Film", "Films", "/films"))
}

pub(super) async fn handle_anime_films() -> Html<String> {
    browse_page(
        "Anime Films",
        "anime-films",
        category_ctx("Anime Film", "Anime Films", "/anime-films"),
    )
}

pub(super) async fn handle_series_root() -> Html<String> {
    browse_page(
        "Series",
        "series",
        json!({
            "category": "Series",
            "view": "series_list",
            "seriesName": Value::Null,
            "seriesSlug": Value::Null,
            "seasonNumber": Value::Null,
            "breadcrumbs": [{"label": "Series", "href": "/series"}],
        }),
    )
}

pub(super) async fn handle_anime_tv_root() -> Html<String> {
    browse_page(
        "Anime TV",
        "anime-tv",
        json!({
            "category": "Anime TV",
            "view": "series_list",
            "seriesName": Value::Null,
            "seriesSlug": Value::Null,
            "seasonNumber": Value::Null,
            "breadcrumbs": [{"label": "Anime TV", "href": "/anime-tv"}],
        }),
    )
}

pub(super) async fn handle_series_path(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Response {
    series_path_response(state, "Series", "/series", "Series", "series", path).await
}

pub(super) async fn handle_anime_tv_path(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Response {
    series_path_response(state, "Anime TV", "/anime-tv", "Anime TV", "anime-tv", path).await
}

pub(super) async fn handle_upload_page() -> Html<String> {
    base_shell(
        "Upload - Telegram HLS Streamer",
        "upload",
        upload_body(),
        "",
        r#"<script src="/static/upload.js?v=5"></script>"#,
    )
}

pub(super) async fn handle_settings_page() -> Html<String> {
    base_shell(
        "Settings - Telegram HLS Streamer",
        "settings",
        settings_body(),
        "",
        r#"<script src="/static/settings.js?v=6"></script>"#,
    )
}

pub(super) async fn handle_watch_page(Path(job_id): Path<String>) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    base_shell(
        "Watch - Telegram HLS Streamer",
        "",
        watch_body(),
        r#"<link rel="stylesheet" href="/static/shaka-controls.css">"#,
        r#"<script src="/static/shaka-player.ui.js"></script><script src="/static/watch.js?v=7"></script>"#,
    )
    .into_response()
}

// ─── series path helper (unchanged) ─────────────────────────────────
async fn series_path_response(
    state: Arc<AppState>,
    category: &str,
    root_href: &str,
    root_label: &str,
    active_sidebar: &str,
    path: String,
) -> Response {
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let Some(slug) = parts.next() else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "series not found");
    };
    let suffix = parts.next();
    if parts.next().is_some() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "page not found");
    }
    let series_name = match resolve_series_slug(&state, category, slug).await {
        Ok(Some(name)) => name,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "series not found"),
        Err(response) => return response,
    };
    let base = format!("{root_href}/{slug}");
    let ctx = match suffix {
        None => json!({
            "category": category,
            "view": "seasons",
            "seriesName": series_name,
            "seriesSlug": slug,
            "seasonNumber": Value::Null,
            "breadcrumbs": [
                {"label": root_label, "href": root_href},
                {"label": series_name, "href": base}
            ],
        }),
        Some("specials") => json!({
            "category": category,
            "view": "episodes",
            "seriesName": series_name,
            "seriesSlug": slug,
            "seasonNumber": Value::Null,
            "breadcrumbs": [
                {"label": root_label, "href": root_href},
                {"label": series_name, "href": base},
                {"label": "Specials", "href": format!("{base}/specials")}
            ],
        }),
        Some(season) => {
            let Some(number) = season.strip_prefix('s').and_then(|s| s.parse::<i64>().ok()) else {
                return api_error(StatusCode::NOT_FOUND, "not_found", "season not found");
            };
            json!({
                "category": category,
                "view": "episodes",
                "seriesName": series_name,
                "seriesSlug": slug,
                "seasonNumber": number,
                "breadcrumbs": [
                    {"label": root_label, "href": root_href},
                    {"label": series_name, "href": base},
                    {"label": format!("Season {number}"), "href": format!("{base}/s{number}")}
                ],
            })
        }
    };
    browse_page(&series_name, active_sidebar, ctx).into_response()
}

async fn resolve_series_slug(
    state: &AppState,
    category: &str,
    slug: &str,
) -> Result<Option<String>, Response> {
    let conn = state.db_conn().await.map_err(db_unavailable)?;
    let category_owned = category.to_string();
    let names = tokio::task::spawn_blocking(move || {
        db::distinct_series_names(&conn, Some(&category_owned))
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!(e)))
    .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()))?;
    Ok(names.into_iter().find(|name| slugify(name) == slug))
}

fn category_ctx(category: &str, label: &str, href: &str) -> Value {
    json!({
        "category": category,
        "view": "grid",
        "seriesName": Value::Null,
        "seriesSlug": Value::Null,
        "seasonNumber": Value::Null,
        "breadcrumbs": [{"label": label, "href": href}],
    })
}

fn browse_page(title: &str, active_tab: &str, ctx: Value) -> Html<String> {
    let context = format!(
        r#"<script>window.BROWSE_CTX = {};</script>"#,
        serde_json::to_string(&ctx).expect("serialise browse ctx")
    );
    base_shell(
        &format!("{title} - Telegram HLS Streamer"),
        active_tab,
        browse_body(),
        "",
        &format!(
            "{context}\
             <script src=\"/static/browse-home.js?v=5\"></script>\
             <script src=\"/static/browse.js?v=5\"></script>"
        ),
    )
}

// ─── base shell — new glass navbar with top tabs ────────────────────
fn base_shell(
    title: &str,
    active_tab: &str,
    body: &str,
    extra_css: &str,
    scripts: &str,
) -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    <script>(function(){{var t=localStorage.getItem('hls_theme');var d=t?t==='dark':window.matchMedia('(prefers-color-scheme: dark)').matches;if(d)document.documentElement.setAttribute('data-theme','dark');else document.documentElement.setAttribute('data-theme','light');}})()</script>
    <link rel="stylesheet" href="https://fonts.googleapis.com/icon?family=Material+Icons+Round">
    <link rel="stylesheet" href="/static/app.css?v=6">
    {extra_css}
</head>
<body>
<nav class="navbar">
    <div class="navbar-left">
        <a class="logo" href="/"><span>TG</span></a>
        <div class="t-tabs">{tabs}</div>
    </div>
    <div class="navbar-center"></div>
    <div class="navbar-right">
        <input type="hidden" id="searchInput">
        <button class="search-trigger" id="searchTriggerBtn" type="button"
                onclick="window.__thls_palette_open&&window.__thls_palette_open()" title="Search (⌘K)">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <circle cx="11" cy="11" r="6.5"/><path d="m20 20-3.5-3.5"/>
            </svg>
            <span class="search-trigger-text">Search library</span>
            <kbd class="thls-kbd">⌘K</kbd>
        </button>
        <span class="t-livepill" id="thls-status-pill" style="display:none" onclick="window.__thls_toggle_jobs_panel && window.__thls_toggle_jobs_panel()">
            <span class="t-livepill__dot"></span>
            <span id="thls-status-text"></span>
        </span>
        <div class="jobs-panel hidden" id="jobsPanel">
            <div class="jobs-panel__head">
                <span class="jobs-panel__title">Active Jobs</span>
                <button class="jobs-panel__close" onclick="window.__thls_toggle_jobs_panel()">&times;</button>
            </div>
            <div class="jobs-panel__list" id="jobsPanelList">
                <div class="jobs-panel__empty">No active jobs</div>
            </div>
        </div>
        <a href="/upload" class="upload-btn" aria-label="Upload">
            <i class="material-icons-round">add</i>
            <span class="upload-btn-text">Upload</span>
        </a>
        <button class="navbar-icon-btn" id="themeToggleBtn" title="Toggle theme">
            <i class="material-icons-round">contrast</i>
        </button>
        <a href="/settings" class="navbar-icon-btn" title="Settings">
            <i class="material-icons-round">settings</i>
        </a>
    </div>
</nav>
<div class="app-body">
    <aside class="sidebar" id="sidebar" hidden></aside>
    {body}
</div>
<script src="/static/shared.js?v=6"></script>
<script src="/static/browse-palette.js?v=5"></script>
{scripts}
</body>
</html>"#,
        tabs = tabs_html(active_tab),
    ))
}

fn tabs_html(active: &str) -> String {
    let items = [
        ("home", "/", "Home"),
        ("films", "/films", "Films"),
        ("series", "/series", "Series"),
        ("anime-films", "/anime-films", "Anime Films"),
        ("anime-tv", "/anime-tv", "Anime TV"),
    ];
    items
        .iter()
        .map(|(key, href, label)| {
            let cls = if *key == active {
                "t-tab active"
            } else {
                "t-tab"
            };
            let aria = if *key == active {
                r#" aria-current="page""#
            } else {
                ""
            };
            format!(r#"<a class="{cls}"{aria} href="{href}">{label}</a>"#)
        })
        .collect::<Vec<_>>()
        .join("")
}

// browse_body now exposes an optional hero mount point. browse-home.js
// only fills it when BROWSE_CTX.view === "home".
fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}
