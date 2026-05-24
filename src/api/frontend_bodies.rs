pub(super) fn browse_body() -> &'static str {
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
pub(super) fn upload_body() -> &'static str {
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
        <div class="error-msg hidden" id="errorMsg"></div>
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

pub(super) fn settings_body() -> &'static str {
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
    <a class="t-side__item" data-section="settings-metadata"><i class="material-icons-round">info</i> Metadata</a>
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
          <div class="settings-subhead">Active Telegram Bots</div>
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
          <div class="settings-subhead">
            <span>Watcher Configuration</span>
            <div class="subhead-actions">
              <span class="settings-status subhead-status" id="watchSettingsStatus"></span>
              <button class="subhead-save-btn" id="saveWatchSettingsBtn" onclick="saveWatchSettings()">Save</button>
            </div>
          </div>
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
      </div>
    </section>

    <section class="settings-section" id="settings-db" hidden>
      <div class="settings-group">
        <div class="settings-group-head">
          <h2>Database management</h2>
          <div class="settings-group-sub">
            Backup, export, import, and replace the SQLite database library.
          </div>
        </div>

        <div class="t-pane settings-pane">
          <div class="settings-subhead">Backup and export</div>
          <div class="t-settings-row">
            <div>
              <div class="t-settings-row-label">Current database</div>
              <div class="t-settings-row-hint">Download a dated <code>.db</code> snapshot or upload it to every configured Telegram bot.</div>
            </div>
            <div class="settings-field-control" style="flex-wrap:wrap;">
              <button class="action-btn" onclick="backupDatabase()">Backup on server</button>
              <button class="action-btn" onclick="downloadDbExport()">Download .db</button>
              <button class="action-btn" onclick="telegramDbExport()">Upload .db to all bots</button>
              <span class="settings-status" id="dbExportStatus"></span>
            </div>
          </div>
        </div>

        <div class="t-pane settings-pane">
          <div class="settings-subhead">Import library database</div>
          <div class="t-settings-row t-settings-row--stack">
            <div>
              <div class="t-settings-row-label">Merge from local file</div>
              <div class="t-settings-row-hint">Upload a <code>.db</code>, <code>.sqlite</code>, or <code>.sqlite3</code> file. Existing local rows are kept.</div>
            </div>
            <div class="form-group settings-db-file">
              <input class="t-input" type="file" id="dbImportFileInput" accept=".db,.sqlite,.sqlite3,application/vnd.sqlite3,application/octet-stream">
              <div class="settings-actions">
                <button class="action-btn primary" onclick="importDbExportFile()">Import file</button>
                <span class="settings-status" id="dbImportStatus"></span>
              </div>
            </div>
          </div>
        </div>

        <div class="t-pane settings-pane">
          <div class="settings-subhead">Replace SQLite file</div>
          <div class="t-settings-row t-settings-row--stack">
            <div>
              <div class="t-settings-row-label">Replacement file</div>
              <div class="t-settings-row-hint">Destructive: wipes the active library, creates a backup, then loads this file. Service auto-restarts.</div>
            </div>
            <div class="form-group settings-db-file">
              <input class="t-input" type="file" id="databaseFileInput" accept=".db,.sqlite,.sqlite3,application/octet-stream">
              <div class="settings-actions">
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
    <section class="settings-section" id="settings-metadata" hidden></section>
    <section class="settings-section" id="settings-system" hidden></section>
    <section class="settings-section" id="settings-cloudflared" hidden></section>
  </main>
</div>
</main>
<div class="modal-overlay" id="tierModal" onclick="handleTierModalOverlayClick(event)">
    <div class="modal">
        <div class="modal-header">
            <span class="modal-title" id="tierModalTitle">Add ABR tier</span>
            <button class="modal-close" onclick="closeTierModal()">
                <span class="material-icons-round">close</span>
            </button>
        </div>
        <div class="form-group">
            <label class="form-label" for="tierHeightInput">Height</label>
            <input class="form-input" type="number" id="tierHeightInput" min="1" placeholder="720">
            <div class="field-description">Video height in pixels.</div>
        </div>
        <div class="form-group">
            <label class="form-label" for="tierBitrateInput">Bitrate</label>
            <input class="form-input" type="text" id="tierBitrateInput" placeholder="5M">
            <div class="field-description">Use FFmpeg-style values like <code>800k</code>, <code>5M</code>, or <code>60M</code>.</div>
        </div>
        <div class="settings-actions">
            <button class="action-btn" onclick="closeTierModal()">Cancel</button>
            <button class="action-btn primary" onclick="saveTierModal()">Save tier</button>
            <span class="settings-status" id="tierModalStatus"></span>
        </div>
    </div>
</div>
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

pub(super) fn watch_body() -> &'static str {
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
        <section id="animeCommunityComments" style="max-width:1200px;margin:32px auto 0;border-radius:12px;overflow:hidden"></section>
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
        <div class="modal-body" style="padding:0 1rem 1rem; border-top:1px solid var(--t-border);">
            <div style="margin:1rem 0 0.75rem; font-size:12px; font-weight:600; letter-spacing:.06em; text-transform:uppercase; color:var(--t-ink-3);">Link External Metadata</div>
            <div style="display:flex; gap:0.5rem; margin-bottom:0.75rem;">
                <select id="metaProvider" class="form-input" style="width:110px; flex-shrink:0;">
                    <option value="tmdb">TMDB</option>
                    <option value="anilist">AniList</option>
                </select>
                <input type="text" id="metaSearchQuery" class="form-input" style="flex:1;" placeholder="Search title…" onkeydown="if(event.key==='Enter')searchExternalMetadata()">
                <button class="modal-btn" onclick="searchExternalMetadata()" id="metaSearchBtn">Search</button>
            </div>
            <div id="metaSearchResults" style="max-height:220px; overflow-y:auto; display:none;"></div>
            <div id="metaLinkedInfo" style="font-size:13px; color:var(--t-ink-2); margin-top:0.5rem;"></div>
        </div>
    </div>
</div>"#
}
