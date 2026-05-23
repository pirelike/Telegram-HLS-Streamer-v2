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
        r#"<script src="/static/settings.js?v=5"></script>"#,
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
        r#"<script src="/static/shaka-player.ui.js"></script><script src="/static/watch.js?v=5"></script>"#,
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
    let names = db::distinct_series_names(&conn, Some(category))
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
    <link rel="stylesheet" href="/static/app.css?v=5">
    {extra_css}
</head>
<body>
<nav class="navbar">
    <div class="navbar-left">
        <a class="logo" href="/"><span>TG</span></a>
        <div class="t-tabs">{tabs}</div>
    </div>
    <div class="navbar-center">
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
    </div>
    <div class="navbar-right">
        <span class="t-livepill" id="thls-status-pill" style="display:none">
            <span class="t-livepill__dot"></span>
            <span id="thls-status-text"></span>
        </span>
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
<script src="/static/shared.js?v=5"></script>
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
fn browse_body() -> &'static str {
    r#"<main class="main" id="mainContent">
    <div id="thlsHero"></div>
    <div class="browse-view" id="browseView">
        <div id="videosContainer"></div>
        <button class="load-more-btn" id="loadMoreBtn" onclick="loadMoreJobs()">Load more</button>
    </div>
</main>
<div class="modal-overlay" id="editModal">
    <div class="modal" style="max-width: 500px;">
        <div class="modal-header">
            <span class="modal-title">Edit Metadata</span>
            <button class="modal-close" onclick="closeEditModal()">
                <i class="material-icons-round">close</i>
            </button>
        </div>
        <div class="modal-body" style="padding: 1rem;">
            <input type="hidden" id="editJobId">
            <div style="margin-bottom: 1rem;">
                <label class="form-label">Title</label>
                <input type="text" id="editTitle" class="form-input" style="width:100%;">
            </div>
            <div style="margin-bottom: 1rem;">
                <label class="form-label">Category</label>
                <select id="editCategory" class="form-input" style="width:100%;" onchange="updateEditModalFields()">
                    <option value="Film">Film</option>
                    <option value="Film Series">Film Series</option>
                    <option value="TV Series">TV Series</option>
                    <option value="Anime Film">Anime Film</option>
                    <option value="Anime TV">Anime TV</option>
                    <option value="Anime TV Series">Anime TV Series</option>
                </select>
            </div>
            <div style="margin-bottom: 1rem;" id="editSeriesGroup">
                <label class="form-label">Series Name</label>
                <input type="text" id="editSeriesName" class="form-input" style="width:100%;">
            </div>
            <div style="display:flex; gap:1rem; margin-bottom: 1rem;">
                <div style="flex:1;" id="editSeasonGroup">
                    <label class="form-label">Season</label>
                    <input type="number" id="editSeasonNumber" class="form-input" style="width:100%;">
                </div>
                <div style="flex:1;" id="editEpisodeGroup">
                    <label class="form-label">Episode</label>
                    <input type="number" id="editEpisodeNumber" class="form-input" style="width:100%;">
                </div>
                <div style="flex:1;" id="editPartGroup">
                    <label class="form-label">Part #</label>
                    <input type="number" id="editPartNumber" class="form-input" style="width:100%;">
                </div>
            </div>
            <div style="display:flex; justify-content:flex-end; gap:0.5rem; margin-top:1.5rem;">
                <button class="modal-btn" onclick="closeEditModal()">Cancel</button>
                <button class="modal-btn primary" id="saveEditBtn" onclick="saveEditModal()">Save Changes</button>
            </div>
        </div>
    </div>
</div>"#
}

// ─── upload_body / settings_body / watch_body (unchanged inner DOM) ─
// Bodies kept structurally identical so upload.js / settings.js / watch.js
// continue to find their IDs. Visual changes come from app.css.
fn upload_body() -> &'static str {
    r##"<main class="main upload-page" id="mainContent">
    <div class="page-card">
        <div class="page-card-header"><span class="page-card-title">Upload Video</span></div>
        <div class="resume-banner hidden" id="resumeBanner">
            <span class="resume-banner-text" id="resumeBannerText"></span>
            <button class="action-btn" onclick="dismissResume()">Dismiss</button>
        </div>
        <div class="segmented-control" id="categoryControl">
            <button class="seg-btn active" data-cat="Film">Film</button>
            <button class="seg-btn" data-cat="Film Series">Film Series</button>
            <button class="seg-btn" data-cat="TV Series">TV Series</button>
            <button class="seg-btn" data-cat="Anime Film">Anime Film</button>
            <button class="seg-btn" data-cat="Anime TV">Anime TV</button>
            <button class="seg-btn" data-cat="Anime TV Series">Anime TV Series</button>
        </div>
        <div class="drop-zone" id="uploadArea">
            <input type="file" id="fileInput" accept="video/*,.mkv,.avi,.mp4,.mov,.webm,.ts,.m4v,.flv">
            <input type="file" id="folderInput" webkitdirectory multiple style="display:none">
            <div class="drop-icon"><i class="material-icons-round">movie</i></div>
            <div class="drop-text" id="dropText">Drop your video here or <strong>click to browse</strong>
                <small>Supports large files — MKV, MP4, AVI, MOV, WebM — Resumable</small>
            </div>
        </div>
        <div style="text-align:center;margin-top:12px;margin-bottom:24px;">
            <button type="button" class="folder-upload-btn hidden" id="folderUploadBtn" onclick="document.getElementById('folderInput').click()">
                <i class="material-icons-round" style="font-size:1.1rem;vertical-align:middle;margin-right:0.25rem;">folder_open</i> Upload Folder
            </button>
        </div>
        <div class="metadata-section hidden" id="metadataSection">
            <div class="apply-all-row hidden" id="applyAllRow">
                <span class="apply-all-label" id="applyAllLabel">Series name:</span>
                <input class="form-input apply-all-input" type="text" id="applyAllInput" placeholder="Apply to all rows">
                <button class="action-btn apply-all-btn" id="applyAllBtn">Apply</button>
            </div>
            <div class="metadata-table-wrap" id="metadataTableWrap"></div>
            <button class="action-btn primary start-upload-btn" id="startUploadBtn" disabled>Start Upload</button>
        </div>
        <div class="error-msg" id="errorMsg"></div>
        <div class="analysis-card hidden" id="analysisCard">
            <h4 style="margin:0 0 10px;font-size:13px;color:var(--t-ink-3);">Detected Streams</h4>
            <div class="stream-badges" id="streamBadges"></div>
        </div>
        <div class="progress-block hidden" id="progressContainer">
            <div class="status-text" id="statusText">Preparing...</div>
            <div class="progress-bar-bg"><div class="progress-bar" id="progressBar"></div></div>
            <div class="progress-info"><span id="progressStep">-</span><span id="progressPct">0%</span></div>
            <div class="speed-text" id="speedText"></div>
            <div class="activity-log" id="activityLog"></div>
            <button class="cancel-btn" id="cancelBtn" onclick="cancelUpload()">Cancel</button>
        </div>
        <div class="result-block hidden" id="resultCard">
            <h4 style="margin:0 0 10px;font-size:14px;"><i class="material-icons-round" style="vertical-align:middle;margin-right:0.3rem;color:var(--t-success);">check_circle</i> Stream Ready</h4>
            <div class="url-box">
                <span class="url-text" id="masterUrl"></span>
                <button class="copy-btn" onclick="copyUrl()">Copy</button>
            </div>
            <a class="watch-link" id="watchLink" href="#"><i class="material-icons-round">play_circle</i> Watch Now</a>
        </div>
    </div>
</main>"##
}

fn settings_body() -> &'static str {
    r#"<main class="main settings-page" id="mainContent">
<div class="t-settings-layout">
  <aside class="t-side t-settings-side" id="settingsSide">
    <div class="t-side__group">Server</div>
    <a class="t-side__item" data-section="settings-server"   aria-current="page"><i class="material-icons-round">settings</i> General</a>
    <a class="t-side__item" data-section="settings-bots"><i class="material-icons-round">smart_toy</i> Telegram bots</a>
    <a class="t-side__item" data-section="settings-watch"><i class="material-icons-round">folder_open</i> Watch folder</a>
    <a class="t-side__item" data-section="settings-db"><i class="material-icons-round">storage</i> Database</a>
    <div class="t-side__group">Media</div>
    <a class="t-side__item" data-section="settings-media"><i class="material-icons-round">movie_filter</i> Transcoding</a>
    <a class="t-side__item" data-section="settings-abr"><i class="material-icons-round">auto_awesome</i> ABR tiers</a>
    <a class="t-side__item" data-section="settings-cache"><i class="material-icons-round">memory</i> Storage</a>
    <div class="t-side__group">System</div>
    <a class="t-side__item" data-section="settings-system"><i class="material-icons-round">tune</i> System</a>
    <a class="t-side__item" data-section="settings-cloudflared"><i class="material-icons-round">cloud</i> Cloudflared</a>
  </aside>

  <main class="t-settings-main t-scroll" id="settingsMain">
    <div class="settings-header">
      <h1 class="settings-title" id="settingsHeading">General</h1>
      <div class="settings-subtitle" id="settingsSubtitle">Streamer settings</div>
    </div>

    <section class="settings-section" id="settings-server"></section>

    <section class="settings-section" id="settings-bots" hidden>
      <div class="settings-group">
        <div class="settings-group-head">
          <h2>Telegram bots</h2>
          <div class="settings-group-sub">Bots from .env cannot be deleted via the UI. Changes take effect immediately.</div>
        </div>
        <div class="t-pane settings-pane">
          <div id="botListContainer" style="padding:8px 0">
            <div class="bot-empty" style="color:var(--t-ink-3);font-size:13px;padding:14px 18px">Loading bots…</div>
          </div>
        </div>
        <div class="settings-actions">
          <button class="action-btn primary" onclick="openAddBotModal()">
            <span class="material-icons-round" style="font-size:1.1rem;vertical-align:middle;">add</span> Add bot
          </button>
          <button class="action-btn" onclick="checkAllBotHealth()">Check all health</button>
          <span class="settings-status" id="botHealthStatus"></span>
        </div>
      </div>
    </section>

    <section class="settings-section" id="settings-watch" hidden>
      <div class="settings-group">
        <div class="settings-group-head">
          <h2>Watch folder</h2>
          <div class="settings-group-sub">Scan a directory for new media files and auto-ingest them.</div>
        </div>
        <div class="t-pane settings-pane">
          <div class="t-settings-row">
            <div>
              <div class="t-settings-row-label">Enable watcher</div>
              <div class="t-settings-row-hint">Background scan every 30 seconds</div>
            </div>
            <div><button class="t-switch" id="watchEnabledSwitch" role="switch" aria-checked="false"></button></div>
            <input type="checkbox" id="watchEnabled" hidden>
          </div>
          <div class="t-settings-row">
            <div>
              <div class="t-settings-row-label">Watch root</div>
              <div class="t-settings-row-hint">Directory to scan</div>
            </div>
            <div><input class="t-input" id="watchRoot" placeholder="/path/to/incoming" style="width:100%;max-width:420px"></div>
          </div>
          <div class="t-settings-row">
            <div>
              <div class="t-settings-row-label">Done directory</div>
              <div class="t-settings-row-hint">Where ingested files are moved</div>
            </div>
            <div><input class="t-input" id="watchDoneDir" placeholder="/path/to/incoming/done" style="width:100%;max-width:420px"></div>
          </div>
        </div>
        <div class="settings-actions">
          <button class="action-btn primary" id="saveWatchSettingsBtn" onclick="saveWatchSettings()">Save</button>
          <span class="settings-status" id="watchSettingsStatus"></span>
        </div>
      </div>
    </section>

    <section class="settings-section" id="settings-db" hidden>
      <div class="settings-group">
        <div class="settings-group-head">
          <h2>Backup & restore</h2>
          <div class="settings-group-sub">
            Two ways to sync libraries between servers: export the database as JSON
            (re-uses Telegram media that's already uploaded) or replace the whole SQLite file.
          </div>
        </div>
        <div class="t-pane settings-pane">
          <div class="t-settings-row">
            <div>
              <div class="t-settings-row-label">Export current database</div>
              <div class="t-settings-row-hint">Snapshot the SQLite file or generate a JSON for another server to import.</div>
            </div>
            <div class="settings-actions" style="margin:0;padding:0;border:0;gap:8px;flex-wrap:wrap;">
              <button class="action-btn" onclick="backupDatabase()">Backup on server</button>
              <button class="action-btn" onclick="downloadDbExport()">Download JSON</button>
              <button class="action-btn" onclick="telegramDbExport()">Push JSON via bot</button>
              <span class="settings-status" id="dbExportStatus"></span>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-group">
        <div class="settings-group-head">
          <h2>Import library JSON</h2>
          <div class="settings-group-sub">
            Pick the source: <b>upload</b> a JSON from disk, or have one of your bots <b>fetch</b> a JSON
            previously pushed to Telegram. The optional bot map only matters if the source server's bot
            line-up differs from this one — leave it empty for auto-mapping.
          </div>
        </div>
        <div class="t-pane settings-pane">
          <div class="t-settings-row t-settings-row--stack">
            <div>
              <div class="t-settings-row-label">From local file</div>
              <div class="t-settings-row-hint">Upload a JSON file from your computer.</div>
            </div>
            <div class="form-group">
              <input class="t-input" type="file" id="dbImportFileInput" accept="application/json,.json">
              <label class="form-label" for="dbImportMap" style="margin-top:10px">Bot map (optional)</label>
              <textarea class="t-input" id="dbImportMap" rows="2"
                        placeholder='e.g. {"0": 1, "1": 2} — leave empty to auto-map all to bot 0'></textarea>
              <div class="settings-actions" style="margin-top:10px;padding-top:0;border-top:none;">
                <button class="action-btn primary" onclick="importDbExportFile()">Import file</button>
                <span class="settings-status" id="dbImportStatus"></span>
              </div>
            </div>
          </div>
          <div class="t-settings-row t-settings-row--stack">
            <div>
              <div class="t-settings-row-label">From Telegram</div>
              <div class="t-settings-row-hint">Auto-filled after a successful "Push JSON via bot" above.</div>
            </div>
            <div class="form-group">
              <label class="form-label" for="telegramImportFileId">Telegram <code>file_id</code></label>
              <input class="t-input" id="telegramImportFileId" autocomplete="off">
              <label class="form-label" for="telegramImportBotIndex" style="margin-top:10px">Downloader bot index</label>
              <input class="t-input" id="telegramImportBotIndex" type="number" value="0" min="0" style="width:140px">
              <div class="settings-actions" style="margin-top:10px;padding-top:0;border-top:none;">
                <button class="action-btn" onclick="importDbExportTelegram()">Import from Telegram</button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="settings-group">
        <div class="settings-group-head">
          <h2>Replace SQLite file</h2>
          <div class="settings-group-sub">
            Wipes everything and loads a different <code>streamer.db</code>. A backup is created automatically before replacement.
          </div>
        </div>
        <div class="t-pane settings-pane">
          <div class="t-settings-row t-settings-row--stack">
            <div>
              <div class="t-settings-row-label">Database file</div>
              <div class="t-settings-row-hint">A <code>.db</code>, <code>.sqlite</code>, or <code>.sqlite3</code> file. Service auto-restarts.</div>
            </div>
            <div class="form-group">
              <input class="t-input" type="file" id="databaseFileInput" accept=".db,.sqlite,.sqlite3,application/octet-stream">
              <div class="settings-actions" style="margin-top:10px;padding-top:0;border-top:none;">
                <button class="action-btn danger" id="databaseLoadBtn" onclick="loadDatabaseFromFile()">Load database</button>
                <span class="settings-status" id="databaseLoadStatus"></span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>

    <section class="settings-section" id="settings-media" hidden></section>
    <section class="settings-section" id="settings-abr" hidden></section>
    <section class="settings-section" id="settings-cache" hidden></section>
    <section class="settings-section" id="settings-system" hidden></section>
    <section class="settings-section" id="settings-cloudflared" hidden></section>
  </main>
</div>
</main>
<div class="modal-overlay" id="addBotModal" onclick="handleModalOverlayClick(event)">
    <div class="modal">
        <div class="modal-header">
            <span class="modal-title">Add Telegram Bot</span>
            <button class="modal-close" onclick="closeAddBotModal()">
                <span class="material-icons-round">close</span>
            </button>
        </div>
        <div class="form-group">
            <label class="form-label" for="newBotToken">Bot Token</label>
            <input class="form-input" type="text" id="newBotToken" placeholder="123456789:ABCdefGHIjklMNOpqrSTUvwXYZ012345678" autocomplete="off">
            <div class="field-description">Get a token from @BotFather on Telegram.</div>
        </div>
        <div class="form-group">
            <label class="form-label" for="newBotChannelId">Channel ID</label>
            <input class="form-input" type="text" id="newBotChannelId" placeholder="-1001234567890">
            <div class="field-description">Must be a negative integer.</div>
        </div>
        <div class="form-group">
            <label class="form-label" for="newBotLabel">Label <span style="font-weight:400;color:var(--t-ink-3)">(optional)</span></label>
            <input class="form-input" type="text" id="newBotLabel" placeholder="e.g. Main storage bot">
        </div>
        <div class="settings-actions">
            <button class="action-btn primary" id="addBotSaveBtn" onclick="testAndSaveBot()">Test &amp; Save</button>
            <span class="settings-status" id="addBotStatus"></span>
        </div>
    </div>
</div>"#
}

fn watch_body() -> &'static str {
    r#"<main class="main watch-page" id="mainContent">
    <div class="player-view active">
        <div class="player-container" id="playerContainer">
            <video id="videoEl" autoplay playsinline crossorigin="anonymous"></video>
        </div>
        <div class="breadcrumb" id="watchBreadcrumb"></div>
        <section class="t-watch-meta-grid" id="watchMetaGrid">
            <div class="t-watch-meta-main" id="watchMetaMain"></div>
            <aside class="t-pane t-watch-file-details" id="watchFileDetails"></aside>
        </section>
        <div id="episodeNav"></div>
        <div class="player-info" id="playerInfo" hidden></div>
        <section class="t-section t-watch-more" id="watchMoreLikeThis"></section>
    </div>
</main>
<div class="modal-overlay" id="editModal">
    <div class="modal" style="max-width: 500px;">
        <div class="modal-header">
            <span class="modal-title">Edit Metadata</span>
            <button class="modal-close" onclick="closeEditModal()">
                <i class="material-icons-round">close</i>
            </button>
        </div>
        <div class="modal-body" style="padding: 1rem;">
            <input type="hidden" id="editJobId">
            <div style="margin-bottom: 1rem;">
                <label class="form-label">Title</label>
                <input type="text" id="editTitle" class="form-input" style="width:100%;">
            </div>
            <div style="margin-bottom: 1rem;">
                <label class="form-label">Category</label>
                <select id="editCategory" class="form-input" style="width:100%;" onchange="updateEditModalFields()">
                    <option value="Film">Film</option>
                    <option value="Film Series">Film Series</option>
                    <option value="TV Series">TV Series</option>
                    <option value="Anime Film">Anime Film</option>
                    <option value="Anime TV">Anime TV</option>
                    <option value="Anime TV Series">Anime TV Series</option>
                </select>
            </div>
            <div style="margin-bottom: 1rem;" id="editSeriesGroup">
                <label class="form-label">Series Name</label>
                <input type="text" id="editSeriesName" class="form-input" style="width:100%;">
            </div>
            <div style="display:flex; gap:1rem; margin-bottom: 1rem;">
                <div style="flex:1;" id="editSeasonGroup">
                    <label class="form-label">Season</label>
                    <input type="number" id="editSeasonNumber" class="form-input" style="width:100%;">
                </div>
                <div style="flex:1;" id="editEpisodeGroup">
                    <label class="form-label">Episode</label>
                    <input type="number" id="editEpisodeNumber" class="form-input" style="width:100%;">
                </div>
                <div style="flex:1;" id="editPartGroup">
                    <label class="form-label">Part #</label>
                    <input type="number" id="editPartNumber" class="form-input" style="width:100%;">
                </div>
            </div>
            <div style="display:flex; justify-content:flex-end; gap:0.5rem; margin-top:1.5rem;">
                <button class="modal-btn" onclick="closeEditModal()">Cancel</button>
                <button class="modal-btn primary" id="saveEditBtn" onclick="saveEditModal()">Save Changes</button>
            </div>
        </div>
    </div>
</div>"#
}

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
