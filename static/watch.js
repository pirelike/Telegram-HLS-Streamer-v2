// ─── State ────────────────────────────────────────────────────────────────────
let shakaPlayer = null;
let shakaUi = null;
let currentJob = null;
let attemptedQuotaRecovery = false;
let currentSiblings = [];
let currentMarkers = [];
let skipBtn = null;
let skipBtnAutoFade = null;
let playerEventAbort = null;

// ─── Watch progress persistence (used by home "Continue Watching") ────────────
const THLS_PROGRESS_KEY = 'thls_progress_v1';
const THLS_CLIENT_ID_KEY = 'thls_client_id_v1';

function getClientId() {
    let id = null;
    try { id = localStorage.getItem(THLS_CLIENT_ID_KEY); } catch {}
    if (!id) {
        id = 'c' + crypto.randomUUID().replace(/-/g, '');
        try { localStorage.setItem(THLS_CLIENT_ID_KEY, id); } catch {}
    }
    return id;
}

function loadProgressMap() {
    try { return JSON.parse(localStorage.getItem(THLS_PROGRESS_KEY) || '{}') || {}; }
    catch { return {}; }
}
function saveProgress(jobId, seconds, duration) {
    if (!jobId || !duration || duration < 5) return;
    const pct = Math.max(0, Math.min(100, Math.round((seconds / duration) * 100)));
    const map = loadProgressMap();
    if (pct >= 95 || pct <= 1) {
        delete map[jobId];                              // treat as done / not started
    } else {
        map[jobId] = { pct, seconds: Math.floor(seconds), duration: Math.floor(duration), ts: Date.now() };
    }
    try { localStorage.setItem(THLS_PROGRESS_KEY, JSON.stringify(map)); } catch {}
}

async function serverSaveProgress(jobId, seconds, duration) {
    const clientId = getClientId();
    try {
        await fetch(`/api/playback/progress/${encodeURIComponent(jobId)}`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                client_id: clientId,
                position_seconds: Math.max(0, seconds),
                duration_seconds: Math.max(1, duration)
            })
        });
    } catch {}
}

async function serverLoadProgress(jobId) {
    const clientId = getClientId();
    try {
        const resp = await fetch(`/api/playback/progress/${encodeURIComponent(jobId)}?client_id=${encodeURIComponent(clientId)}`);
        if (!resp.ok) return null;
        const data = await resp.json();
        return data.progress;
    } catch { return null; }
}

window.THLSProgress = { load: loadProgressMap, save: saveProgress };
window.THLSClient = { id: getClientId, save: serverSaveProgress, load: serverLoadProgress };

// ─── Intro/outro skip markers ─────────────────────────────────────────────────
async function loadMarkers(jobId) {
    try {
        const resp = await fetch(`/api/jobs/${encodeURIComponent(jobId)}/markers`);
        if (!resp.ok) return [];
        const data = await resp.json();
        return (data.markers || []).filter(m => m.enabled);
    } catch { return []; }
}

function ensureSkipButton() {
    if (skipBtn) return;
    skipBtn = document.createElement('button');
    skipBtn.className = 'skip-intro-btn';
    skipBtn.textContent = 'Skip intro';
    skipBtn.addEventListener('click', () => {
        const marker = currentMarkers.find(m => isInsideMarker(m));
        if (marker && shakaPlayer) {
            const video = document.getElementById('videoEl');
            if (video) video.currentTime = marker.end_seconds;
            skipBtn.classList.remove('visible');
        }
    });
    const container = document.getElementById('playerContainer');
    if (container) container.appendChild(skipBtn);
}

function isInsideMarker(marker) {
    const video = document.getElementById('videoEl');
    if (!video || !marker) return false;
    const t = video.currentTime;
    return t >= marker.start_seconds && t <= marker.end_seconds;
}

function updateSkipButton() {
    const marker = currentMarkers.find(m => isInsideMarker(m));
    if (!marker) {
        if (skipBtn) skipBtn.classList.remove('visible');
        return;
    }
    ensureSkipButton();
    const label = marker.marker_type === 'outro' || marker.marker_type === 'credits'
        ? 'Skip credits'
        : 'Skip intro';
    skipBtn.textContent = label;
    skipBtn.classList.add('visible');
    clearTimeout(skipBtnAutoFade);
    skipBtnAutoFade = setTimeout(() => {
        if (skipBtn) skipBtn.classList.remove('visible');
    }, 15000);
}

// ─── Player init ──────────────────────────────────────────────────────────────
function isMobile() {
    return /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent || '');
}

function getBufferConfig(overrideBufferConfig = null) {
    if (overrideBufferConfig) return overrideBufferConfig;
    if (isMobile()) {
        return {
            bufferingGoal: 5,
            rebufferingGoal: 1,
            bufferBehind: 5,
            defaultBandwidthEstimate: 5_000_000,
        };
    }
    return {
        bufferingGoal: 15,
        rebufferingGoal: 2,
        bufferBehind: 20,
        defaultBandwidthEstimate: 10_000_000,
    };
}

async function initPlayer(job, overrideBufferConfig = null) {
    currentJob = job;
    attemptedQuotaRecovery = Boolean(overrideBufferConfig);
    const videoEl = document.getElementById('videoEl');
    const m3u8Url = `${window.location.origin}/hls/${job.job_id}/master.m3u8`;
    const bufferConfig = getBufferConfig(overrideBufferConfig);

    if (shakaUi) { shakaUi.destroy(); shakaUi = null; }
    if (shakaPlayer) { await shakaPlayer.destroy(); shakaPlayer = null; }

    shaka.polyfill.installAll();
    if (!shaka.Player.isBrowserSupported()) {
        document.getElementById('playerInfo').innerHTML =
            `<p style="color:var(--danger)">Your browser does not support Shaka Player. Use the M3U8 URL directly.</p>`;
    } else {
        const container = document.getElementById('playerContainer');
        const player = new shaka.Player();
        await player.attach(videoEl);
        shakaPlayer = player;

        shakaUi = new shaka.ui.Overlay(player, container, videoEl);
        shakaUi.configure({
            addSeekBar: true,
            addBigPlayButton: true,
            controlPanelElements: [
                'play_pause',
                'mute',
                'volume',
                'spacer',
                'time_and_duration',
                'overflow_menu',
                'fullscreen',
            ],
            overflowMenuButtons: [
                'quality',
                'language',
                'captions',
                'playback_rate',
                'picture_in_picture',
            ],
            seekBarColors: {
                base: 'rgba(255,255,255,0.3)',
                buffered: 'rgba(255,255,255,0.54)',
                played: 'rgb(255,255,255)',
            },
        });

        player.configure({
            streaming: {
                bufferingGoal: bufferConfig.bufferingGoal,
                rebufferingGoal: bufferConfig.rebufferingGoal,
                bufferBehind: bufferConfig.bufferBehind,
                jumpLargeGaps: true,
                smallGapLimit: 0.5,
            },
            abr: {
                defaultBandwidthEstimate: bufferConfig.defaultBandwidthEstimate,
            },
            preferredAudioLanguage: 'und',
            preferredTextLanguage: '',
        });

        player.addEventListener('error', async e => {
            console.error('Shaka error', e.detail);
            const code = e?.detail?.code;
            const message = e?.detail?.message || String(code);

            if (code === 3017 && !attemptedQuotaRecovery) {
                const resumeAt = videoEl.currentTime || 0;
                try {
                    await initPlayer(job, {
                        bufferingGoal: 4,
                        rebufferingGoal: 1,
                        bufferBehind: 4,
                        defaultBandwidthEstimate: 5_000_000,
                    });
                    document.getElementById('playerInfo').insertAdjacentHTML('afterbegin',
                        `<p style="color:var(--warning);margin-bottom:0.5rem">Playback buffer was full (Shaka 3017). Reinitialized with a smaller buffer.</p>`);
                    if (resumeAt > 0 && shakaPlayer) {
                        videoEl.currentTime = resumeAt;
                    }
                    return;
                } catch (retryErr) {
                    console.error('Shaka quota recovery reinit failed', retryErr);
                }
            }

            document.getElementById('playerInfo').insertAdjacentHTML('afterbegin',
                `<p style="color:var(--danger);margin-bottom:0.5rem">Playback error: ${escapeHtml(message)}</p>`);
            if (code === 3014 || code === 3015) {
                document.getElementById('playerInfo').insertAdjacentHTML('afterbegin',
                    `<p style="color:var(--text-muted);margin-bottom:0.5rem">Tip: this usually means the HLS manifest codec tag does not match the actual video stream, or your device does not support the container format. Re-process the video if this job was created before the fMP4 container fix.</p>`);
            }
            if (code === 3017) {
                document.getElementById('playerInfo').insertAdjacentHTML('afterbegin',
                    `<p style="color:var(--text-muted);margin-bottom:0.5rem">Tip: this usually means one or more HLS segments are too large for browser MSE memory. Re-process this video with smaller segments or disable copy mode.</p>`);
            }
        });

        renderInfoPanel(job);

        // Load intro/outro markers for skip UI.
        currentMarkers = await loadMarkers(job.job_id);

        // Resume from saved progress + persist while playing.
        const saved = (await serverLoadProgress(job.job_id)) || loadProgressMap()[job.job_id];
        const resumeSeconds = (() => {
            const seconds = Number(saved?.position_seconds ?? saved?.seconds ?? 0);
            const knownDuration = Number(job.duration || 0);
            if (!Number.isFinite(seconds) || seconds <= 5) return 0;
            if (knownDuration > 0 && knownDuration <= seconds + 5) return 0;
            return seconds;
        })();
        if (playerEventAbort) playerEventAbort.abort();
        playerEventAbort = new AbortController();
        const playerEventOptions = { signal: playerEventAbort.signal };
        let lastSave = 0;
        let serverLastSave = 0;
        videoEl.addEventListener('timeupdate', () => {
            const now = Date.now();
            updateSkipButton();
            if (now - lastSave < 5000) return;
            lastSave = now;
            saveProgress(job.job_id, videoEl.currentTime, videoEl.duration || job.duration || 0);
        }, playerEventOptions);
        videoEl.addEventListener('timeupdate', () => {
            const now = Date.now();
            if (now - serverLastSave < 30000) return;
            serverLastSave = now;
            serverSaveProgress(job.job_id, videoEl.currentTime, videoEl.duration || job.duration || 0);
        }, playerEventOptions);
        videoEl.addEventListener('ended', () => {
            saveProgress(job.job_id, videoEl.duration, videoEl.duration);
            serverSaveProgress(job.job_id, videoEl.duration, videoEl.duration);
        }, playerEventOptions);
        videoEl.addEventListener('pause', () => {
            serverSaveProgress(job.job_id, videoEl.currentTime, videoEl.duration || job.duration || 0);
        }, playerEventOptions);
        videoEl.addEventListener('seeked', () => {
            serverSaveProgress(job.job_id, videoEl.currentTime, videoEl.duration || job.duration || 0);
        }, playerEventOptions);

        try {
            await player.load(m3u8Url, resumeSeconds || undefined);
        }
        catch (e) {
            console.error('Shaka load error', e);
            document.getElementById('playerInfo').insertAdjacentHTML('afterbegin',
                `<p style="color:var(--danger);margin-bottom:0.5rem">Failed to load stream: ${escapeHtml(e.message || String(e))}</p>`);
        }
    }
}

function renderInfoPanel(job) {
    const main = document.getElementById('watchMetaMain');
    const aside = document.getElementById('watchFileDetails');
    if (!main || !aside) return;

    const audioCount = job.audio_count || 0;
    const subCount   = job.subtitle_count || 0;
    const m3u8Url    = `${window.location.origin}/hls/${job.job_id}/master.m3u8`;
    const safeId     = escapeAttr(job.job_id);

    const ext = job.external_metadata || {};
    const displayTitle = (ext.title && ext.title !== job.filename)
        ? escapeHtml(ext.title)
        : escapeHtml(cleanTitle(job.filename || job.job_id));

    // Backdrop: use external backdrop behind the page hero area
    const backdropUrl = ext.backdrop_url || ext.poster_url || '';
    const heroEl = document.getElementById('watchMetaGrid');
    if (heroEl && backdropUrl) {
        heroEl.style.backgroundImage = `url('${escapeAttr(backdropUrl)}')`;
        heroEl.style.backgroundSize = 'cover';
        heroEl.style.backgroundPosition = 'center top';
        heroEl.classList.add('has-backdrop');
    }

    const eyebrowParts = [
        job.media_type ? escapeHtml(job.media_type) : null,
        ext.year ? String(ext.year) : null,
        job.video_height ? `${job.video_height}p` : null,
        subCount > 0 ? `SUB · ${subCount}` : null,
        audioCount > 1 ? `AUDIO · ${audioCount}` : null,
    ].filter(Boolean);
    const eyebrowHtml = eyebrowParts.map((p, i) =>
        (i > 0 ? '<span class="dot"></span>' : '') + `<span>${p}</span>`
    ).join('');

    const metaParts = [];
    if (job.duration > 0) metaParts.push(formatDuration(job.duration));
    if (job.video_height) metaParts.push(`${job.video_height}p`);
    if (job.video_codec) metaParts.push(job.video_codec);
    if (ext.rating) metaParts.push(`★ ${ext.rating.toFixed(1)}`);
    if (job.series_name && job.is_series) {
        if (job.season_number != null && job.episode_number != null) {
            metaParts.push(`S${String(job.season_number).padStart(2,'0')}E${String(job.episode_number).padStart(2,'0')}`);
        } else if (job.part_number != null) {
            metaParts.push(`Part ${job.part_number}`);
        }
    }
    const metaHtml = metaParts.map((p, i) =>
        (i > 0 ? '<span class="dot"></span>' : '') + `<span>${escapeHtml(p)}</span>`
    ).join('');

    const overviewHtml = ext.overview
        ? `<p class="desc">${escapeHtml(ext.overview)}</p>`
        : (job.series_name ? `<p class="desc" style="opacity:.7">From ${escapeHtml(job.series_name)}.</p>` : '');

    // Saved progress → resume label
    const progressMap = (window.THLSProgress?.load && window.THLSProgress.load()) || {};
    const saved = progressMap[job.job_id];
    const resumeLabel = saved && saved.seconds > 5
        ? `Resume at ${formatDuration(saved.seconds)}` : 'Play';
    const resumeIcon = saved && saved.seconds > 5
        ? '<i class="material-icons-round" style="font-size:18px">play_arrow</i>'
        : '<i class="material-icons-round" style="font-size:18px">play_arrow</i>';

    main.innerHTML = `
        <div class="eyebrow">${eyebrowHtml}</div>
        <h1>${displayTitle}</h1>
        ${metaHtml ? `<div class="meta-row">${metaHtml}</div>` : ''}
        ${overviewHtml}
        <div class="t-watch-actions">
            <button class="t-btn t-btn--primary" onclick="document.getElementById('videoEl').play()">
                ${resumeIcon} ${resumeLabel}
            </button>
            <button class="t-btn t-btn--ghost" onclick="copyPlayerUrl()" title="Copy M3U8">
                <i class="material-icons-round" style="font-size:16px">link</i> Copy M3U8
            </button>
            <button class="t-btn t-btn--ghost" onclick="openEditModal('${safeId}')">
                <i class="material-icons-round" style="font-size:16px">edit</i> Edit
            </button>
            <button class="t-btn t-btn--ghost" onclick="deleteJob('${safeId}')" style="color:#ff6b6b">
                <i class="material-icons-round" style="font-size:16px">delete</i> Delete
            </button>
            <span id="playerM3u8Url" hidden>${escapeHtml(m3u8Url)}</span>
        </div>`;

    const rows = [
        ['Original',  job.filename || job.job_id],
        ['Stored',    [
            job.segment_count ? `${job.segment_count} segments` : null,
            job.total_bytes != null ? formatBytes(job.total_bytes) : null,
        ].filter(Boolean).join(' · ') || '—'],
        ['Codec',     [job.video_codec, job.video_height ? `${job.video_height}p` : null].filter(Boolean).join(' · ') || '—'],
        ['Audio',     audioCount > 0 ? `${audioCount} track${audioCount !== 1 ? 's' : ''}` : '—'],
        ['Subtitles', subCount > 0 ? `${subCount} track${subCount !== 1 ? 's' : ''}` : '—'],
        ['Job ID',    job.job_id],
    ];
    aside.innerHTML =
        `<div class="head">File details</div>` +
        rows.map(([k, v]) =>
            `<div class="row"><span>${escapeHtml(k)}</span><span title="${escapeHtml(String(v))}">${escapeHtml(String(v))}</span></div>`
        ).join('') +
        `<button class="t-btn t-btn--quiet" style="margin-top:14px;width:100%;background:var(--t-surface)"
                 onclick="openEditModal('${safeId}')">Edit metadata</button>`;

    renderMoreLikeThis(job);
}

function renderMoreLikeThis(job) {
    const host = document.getElementById('watchMoreLikeThis');
    if (!host) return;
    const cat = job.media_type || 'Film';
    const url = new URL('/api/jobs', window.location.origin);
    url.searchParams.set('category', cat);
    url.searchParams.set('limit', '12');
    fetch(url)
        .then(r => r.json())
        .then(d => {
            const items = (d.jobs || []).filter(j => j.job_id !== job.job_id).slice(0, 8);
            if (!items.length) { host.innerHTML = ''; return; }
            host.innerHTML =
                `<div class="t-section-head"><div><h2 class="t-section-title">More like this</h2></div></div>` +
                `<div class="t-row">${items.map(j => moreCardHtml(j)).join('')}</div>`;
        })
        .catch(() => { host.innerHTML = ''; });
}
function moreCardHtml(j) {
    const safeId = escapeAttr(j.job_id);
    const title  = escapeHtml(cleanTitle(j.filename || j.job_id));
    const dur    = formatDuration(j.duration);
    const grad   = jobIdToGradient(j.job_id);
    const thumb  = j.has_thumbnail
        ? `<img class="thumb-img" src="/thumbnail/${safeId}" alt="" loading="lazy" onload="this.classList.add('loaded')">`
        : `<div class="thumb-placeholder"><i class="material-icons-round">play_circle_filled</i></div>`;
    return `<a class="video-card" href="/watch/${safeId}">
        <div class="thumb-wrap" style="background:${grad}">
          ${thumb}
          ${dur ? `<div class="thumb-duration">${dur}</div>` : ''}
        </div>
        <div class="card-meta">
          <div class="card-title">${title}</div>
        </div>
      </a>`;
}

// ─── Watch breadcrumb ─────────────────────────────────────────────────────────
function renderWatchBreadcrumb(job) {
    const el = document.getElementById('watchBreadcrumb');
    if (!el) return;
    const catLabels = { Film: 'Films', Series: 'Series', 'Anime Film': 'Anime Films', 'Anime TV': 'Anime TV' };
    const catLabel = catLabels[job.media_type] || job.media_type || 'Home';
    const catPath = CATEGORY_PATHS[job.media_type] || '/';
    const crumbs = [{ label: catLabel, href: catPath }];
    if (job.is_series && job.series_name) {
        if (job.media_type === 'Series' || job.media_type === 'Anime TV') {
            const s = slugify(job.series_name);
            crumbs.push({ label: job.series_name, href: catPath + '/' + s });
            if (job.season_number != null) {
                crumbs.push({ label: `Season ${job.season_number}`, href: catPath + '/' + s + '/s' + job.season_number });
            } else {
                crumbs.push({ label: 'Specials', href: catPath + '/' + s + '/specials' });
            }
        } else {
            crumbs.push({ label: job.series_name, href: null });
        }
    }
    el.innerHTML = crumbs.map((c, i) => {
        const item = c.href
            ? `<a class="breadcrumb-item" href="${escapeAttr(c.href)}">${escapeHtml(c.label)}</a>`
            : `<span class="breadcrumb-item">${escapeHtml(c.label)}</span>`;
        return item + (i < crumbs.length - 1 ? '<i class="material-icons-round">chevron_right</i>' : '');
    }).join('');
}

// ─── Episode navigation ───────────────────────────────────────────────────────
async function fetchSiblings(job) {
    if (!job.is_series || !job.series_name) return [];
    const url = new URL('/api/jobs', window.location.origin);
    url.searchParams.set('series_name', job.series_name);
    url.searchParams.set('category', job.media_type);
    if (job.media_type !== 'Film' && job.season_number != null) {
        url.searchParams.set('season_number', job.season_number);
    }
    url.searchParams.set('limit', '1000');
    try {
        const resp = await fetch(url);
        const data = await resp.json();
        const jobs = data.jobs || [];
        if (job.media_type === 'Film') {
            jobs.sort((a, b) => (a.part_number || 0) - (b.part_number || 0));
        } else {
            jobs.sort((a, b) => (a.episode_number || 0) - (b.episode_number || 0));
        }
        return jobs;
    } catch { return []; }
}

function renderEpisodeNav(job, siblings) {
    const el = document.getElementById('episodeNav');
    if (!el) return;
    if (siblings.length < 2) { el.innerHTML = ''; return; }
    const idx = siblings.findIndex(s => s.job_id === job.job_id);
    if (idx === -1) { el.innerHTML = ''; return; }
    const prev = idx > 0 ? siblings[idx - 1] : null;
    const next = idx < siblings.length - 1 ? siblings[idx + 1] : null;
    if (!prev && !next) { el.innerHTML = ''; return; }

    function epLabel(s) {
        if (job.media_type === 'Film') {
            return s.part_number != null ? `Part ${s.part_number}` : cleanTitle(s.filename || s.job_id);
        }
        if (s.season_number != null && s.episode_number != null) {
            return `S${String(s.season_number).padStart(2,'0')}E${String(s.episode_number).padStart(2,'0')}`;
        }
        if (s.episode_number != null) return `Ep ${s.episode_number}`;
        return cleanTitle(s.filename || s.job_id);
    }

    const prevHtml = prev
        ? `<a class="ep-nav-btn" href="/watch/${escapeAttr(prev.job_id)}">
            <i class="material-icons-round">chevron_left</i>
            <div class="ep-nav-info">
                <span class="ep-nav-dir">Previous</span>
                <span class="ep-nav-ep">${escapeHtml(epLabel(prev))}</span>
            </div>
           </a>`
        : `<div class="ep-nav-btn ep-nav-spacer"></div>`;

    const nextHtml = next
        ? `<a class="ep-nav-btn" href="/watch/${escapeAttr(next.job_id)}">
            <div class="ep-nav-info" style="text-align:right">
                <span class="ep-nav-dir">Next</span>
                <span class="ep-nav-ep">${escapeHtml(epLabel(next))}</span>
            </div>
            <i class="material-icons-round">chevron_right</i>
           </a>`
        : `<div class="ep-nav-btn ep-nav-spacer"></div>`;

    el.innerHTML = `<div class="episode-nav">${prevHtml}${nextHtml}</div>`;
}

function copyPlayerUrl() {
    const el = document.getElementById('playerM3u8Url');
    if (el) navigator.clipboard.writeText(el.textContent).then(() => {
        const btn = document.querySelector('[onclick="copyPlayerUrl()"]');
        if (!btn) return;
        const originalHtml = btn.innerHTML;
        btn.textContent = 'Copied!';
        setTimeout(() => { btn.innerHTML = originalHtml; }, 2000);
    });
}

// ─── Edit Metadata & Delete ───────────────────────────────────────────────────
async function deleteJob(jobId) {
    if (!confirm('Are you sure you want to delete this video? This cannot be undone.')) return;
    try {
        const resp = await fetch(`/api/jobs/${encodeURIComponent(jobId)}`, { method: 'DELETE' });
        if (!resp.ok) {
            const data = await resp.json();
            throw new Error(data.error || 'Failed to delete');
        }
        window.location.href = '/';
    } catch (e) {
        alert(e.message);
    }
}

function closeEditModal() {
    document.getElementById('editModal').classList.remove('active');
    document.getElementById('metaSearchResults').style.display = 'none';
    document.getElementById('metaSearchResults').innerHTML = '';
    document.getElementById('metaSearchQuery').value = '';
    document.getElementById('metaLinkedInfo').textContent = '';
}

function parseDirectMetaId(q, selectedProvider) {
    let m = q.match(/anilist\.co\/(anime|manga|manhwa|novel)\/(\d+)/i);
    if (m) return { provider: 'anilist', id: m[2], kind: m[1].toLowerCase() };
    m = q.match(/themoviedb\.org\/(movie|tv)\/(\d+)/i);
    if (m) return { provider: 'tmdb', id: m[2], kind: m[1].toLowerCase() };
    if (selectedProvider === 'anilist' && /^\d+$/.test(q)) return { provider: 'anilist', id: q, kind: 'anime' };
    return null;
}

async function searchExternalMetadata() {
    const jobId = document.getElementById('editJobId').value;
    const provider = document.getElementById('metaProvider').value;
    const q = document.getElementById('metaSearchQuery').value.trim();
    if (!q) return;

    const direct = parseDirectMetaId(q, provider);
    if (direct) {
        await linkMetadata(jobId, direct.provider, direct.id, direct.kind);
        return;
    }

    const btn = document.getElementById('metaSearchBtn');
    btn.disabled = true;
    btn.textContent = '…';
    const resultsEl = document.getElementById('metaSearchResults');
    try {
        const resp = await fetch(`/api/metadata/search?provider=${encodeURIComponent(provider)}&q=${encodeURIComponent(q)}`);
        const data = await resp.json();
        const items = data.results || [];
        if (!items.length) {
            resultsEl.innerHTML = '<div style="padding:8px;color:var(--t-ink-3);font-size:13px;">No results.</div>';
        } else {
            resultsEl.innerHTML = items.map(r => {
                const title = escapeHtml(r.title || r.original_title || '');
                const year  = r.year ? ` (${r.year})` : '';
                const kind  = r.media_kind ? ` · ${escapeHtml(r.media_kind)}` : '';
                const poster = r.poster_url ? `<img src="${escapeAttr(r.poster_url)}" style="width:32px;height:48px;object-fit:cover;border-radius:3px;flex-shrink:0;" loading="lazy">` : '<div style="width:32px;height:48px;background:var(--t-surface);border-radius:3px;flex-shrink:0;"></div>';
                return `<div style="display:flex;gap:10px;align-items:center;padding:8px 4px;border-bottom:1px solid var(--t-border);cursor:pointer;" onclick="linkMetadata('${escapeAttr(jobId)}','${escapeAttr(r.provider)}','${escapeAttr(r.provider_id)}','${escapeAttr(r.media_kind)}')">
                    ${poster}
                    <div style="min-width:0;">
                        <div style="font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${title}${escapeHtml(year)}</div>
                        <div style="font-size:11px;color:var(--t-ink-3);">${escapeHtml(r.provider.toUpperCase())}${escapeHtml(kind)}</div>
                    </div>
                </div>`;
            }).join('');
        }
        resultsEl.style.display = 'block';
    } catch (e) {
        resultsEl.innerHTML = '<div style="padding:8px;color:#ff6b6b;font-size:13px;">Search failed.</div>';
        resultsEl.style.display = 'block';
    } finally {
        btn.disabled = false;
        btn.textContent = 'Search';
    }
}

async function linkMetadata(jobId, provider, providerId, mediaKind) {
    const infoEl = document.getElementById('metaLinkedInfo');
    infoEl.textContent = 'Linking…';
    try {
        const resp = await fetch(`/api/jobs/${encodeURIComponent(jobId)}/metadata/link`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ provider, provider_id: providerId, media_kind: mediaKind }),
        });
        if (!resp.ok) throw new Error(await resp.text());
        infoEl.style.color = 'var(--t-accent)';
        infoEl.textContent = '✓ Metadata linked. Reload page to see changes.';
        document.getElementById('metaSearchResults').style.display = 'none';
    } catch (e) {
        infoEl.style.color = '#ff6b6b';
        infoEl.textContent = `Failed: ${e.message}`;
    }
}

function updateEditModalFields() {
    const cat = document.getElementById('editCategory').value;
    const seriesGrp = document.getElementById('editSeriesGroup');
    const seasonGrp = document.getElementById('editSeasonGroup');
    const epGrp = document.getElementById('editEpisodeGroup');
    const partGrp = document.getElementById('editPartGroup');

    seriesGrp.style.display = 'none';
    seasonGrp.style.display = 'none';
    epGrp.style.display = 'none';
    partGrp.style.display = 'none';

    if (cat === 'Film Series') {
        seriesGrp.style.display = 'block';
        partGrp.style.display = 'block';
    } else if (['TV Series', 'Anime TV', 'Anime TV Series'].includes(cat)) {
        seriesGrp.style.display = 'block';
        seasonGrp.style.display = 'block';
        epGrp.style.display = 'block';
    }
}

function getCategoryFromJob(job) {
    if (job.media_type === 'Film') return job.is_series ? 'Film Series' : 'Film';
    if (job.media_type === 'Series') return 'TV Series';
    if (job.media_type === 'Anime Film') return 'Anime Film';
    if (job.media_type === 'Anime TV') return job.is_series ? 'Anime TV Series' : 'Anime TV';
    return 'Film';
}

function openEditModal(jobId) {
    if (!currentJob || currentJob.job_id !== jobId) return;

    document.getElementById('editJobId').value = currentJob.job_id;
    document.getElementById('editTitle').value = cleanTitle(currentJob.filename || currentJob.job_id);
    document.getElementById('editCategory').value = getCategoryFromJob(currentJob);
    document.getElementById('editSeriesName').value = currentJob.series_name || '';
    document.getElementById('editSeasonNumber').value = currentJob.season_number != null ? currentJob.season_number : '';
    document.getElementById('editEpisodeNumber').value = currentJob.episode_number != null ? currentJob.episode_number : '';
    document.getElementById('editPartNumber').value = currentJob.part_number != null ? currentJob.part_number : '';

    updateEditModalFields();
    document.getElementById('editModal').classList.add('active');
}

async function saveEditModal() {
    const jobId = document.getElementById('editJobId').value;
    const cat = document.getElementById('editCategory').value;
    const btn = document.getElementById('saveEditBtn');

    const dbFields = CATEGORY_DB[cat];
    const payload = {
        title: document.getElementById('editTitle').value.trim(),
        media_type: dbFields.media_type,
        is_series: dbFields.is_series,
        series_name: document.getElementById('editSeriesName').value.trim()
    };

    if (cat === 'Film Series') {
        payload.part_number = document.getElementById('editPartNumber').value;
    } else if (['TV Series', 'Anime TV', 'Anime TV Series'].includes(cat)) {
        payload.season_number = document.getElementById('editSeasonNumber').value;
        payload.episode_number = document.getElementById('editEpisodeNumber').value;
    }

    btn.disabled = true;
    btn.textContent = 'Saving...';

    try {
        const resp = await fetch(`/api/jobs/${encodeURIComponent(jobId)}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
        });
        if (!resp.ok) {
            const data = await resp.json();
            throw new Error(data.error || 'Failed to save');
        }

        // Update currentJob in-place
        Object.assign(currentJob, payload);
        if (payload.title) currentJob.filename = payload.title;
        if (payload.part_number !== undefined) currentJob.part_number = payload.part_number ? parseInt(payload.part_number) : null;
        if (payload.season_number !== undefined) currentJob.season_number = payload.season_number ? parseInt(payload.season_number) : null;
        if (payload.episode_number !== undefined) currentJob.episode_number = payload.episode_number ? parseInt(payload.episode_number) : null;
        if (!['Film Series'].includes(cat)) currentJob.part_number = null;
        if (!['TV Series', 'Anime TV', 'Anime TV Series'].includes(cat)) {
            currentJob.season_number = null;
            currentJob.episode_number = null;
        }
        if (['Film', 'Anime Film'].includes(cat)) currentJob.series_name = '';

        closeEditModal();
        renderInfoPanel(currentJob);
        renderWatchBreadcrumb(currentJob);
        fetchSiblings(currentJob).then(s => { currentSiblings = s; renderEpisodeNav(currentJob, s); });
    } catch (e) {
        alert(e.message);
    } finally {
        btn.disabled = false;
        btn.textContent = 'Save Changes';
    }
}

// ─── Init ─────────────────────────────────────────────────────────────────────
(async () => {
    // Extract job_id from URL path: /watch/<job_id>
    const jobId = window.location.pathname.split('/watch/')[1];
    if (!jobId) {
        document.getElementById('playerInfo').innerHTML =
            `<p style="color:var(--danger)">No job ID in URL.</p>`;
        return;
    }

    let job;
    try {
        const resp = await fetch(`/api/jobs/${encodeURIComponent(jobId)}`);
        if (!resp.ok) throw new Error('Job not found');
        job = await resp.json();
    } catch (e) {
        document.getElementById('playerInfo').innerHTML =
            `<p style="color:var(--danger)">Could not load video: ${escapeHtml(e.message)}</p>`;
        return;
    }

    renderWatchBreadcrumb(job);
    initPlayer(job); // non-blocking — handles its own errors

    currentSiblings = await fetchSiblings(job);
    renderEpisodeNav(job, currentSiblings);

    renderAnimeCommunityComments(job);
})();

// ─── Anime Community embed ────────────────────────────────────────────────────
let _tacScriptLoaded = false;
function renderAnimeCommunityComments(job) {
    if (job.feature_flags && job.feature_flags.tac_comments_enabled === false) return;
    const isAnime = job.media_type === 'Anime TV' || job.media_type === 'Anime Film';
    if (!isAnime) return;
    const container = document.getElementById('animeCommunityComments');
    if (!container) return;

    const anilistId = job.external_ids?.anilist;
    const malId = job.external_ids?.mal;
    if (!anilistId && !malId) {
        return; // no external id, skip
    }

    const epNum = job.media_type === 'Anime Film'
        ? '0'
        : (job.episode_number != null ? String(job.episode_number) : '1');

    window.theAnimeCommunityConfig = {
        AniList_ID: anilistId ? String(anilistId) : undefined,
        MAL_ID: malId ? String(malId) : undefined,
        episodeChapterNumber: epNum,
        mediaType: 'anime',
        removeBorderStyling: true
    };

    if (window.theAnimeCommunity && window.theAnimeCommunity.reload) {
        window.theAnimeCommunity.reload();
    } else if (!_tacScriptLoaded) {
        _tacScriptLoaded = true;
        container.innerHTML = '<div id="anime-community-comment-section"></div>';
        const script = document.createElement('script');
        script.src = 'https://theanimecommunity.com/embed.js';
        script.id = 'anime-community-script';
        script.onload = () => { console.log('Anime Community comments loaded'); };
        script.onerror = () => { container.innerHTML = ''; };
        document.head.appendChild(script);
    }
}

window.addEventListener('message', function(event) {
    if (event.origin !== 'https://theanimecommunity.com') return;
    if (event.data && event.data.type === 'TAC-TIMESTAMP-CLICK') {
        var time = Number(event.data.time);
        if (Number.isFinite(time)) {
            var video = document.getElementById('videoEl');
            if (video) video.currentTime = time;
        }
    }
});
