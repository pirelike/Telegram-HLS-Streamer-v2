// ─── Constants ────────────────────────────────────────────────────────────────
const JOBS_PER_PAGE = 20;

// ─── State ────────────────────────────────────────────────────────────────────
let allJobs = [];
let searchQuery = '';
let jobsPage = 1;
let hasMoreJobs = false;
let _seriesDetailSelected = null;
let _seriesDetailEpisodes = {};
let _seriesDetailKey = null;

// ─── DOM refs ─────────────────────────────────────────────────────────────────
const searchInput = document.getElementById('searchInput');
const videosContainer = document.getElementById('videosContainer');
const loadMoreBtn = document.getElementById('loadMoreBtn');

// ─── Search ──────────────────────────────────────────────────────────────────
let searchTimeout = null;
function setSearchQuery(value) {
    searchQuery = String(value || '').trim();
    if (searchInput) searchInput.value = searchQuery;
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(loadJobs, 400);
}
if (searchInput) {
    searchInput.addEventListener('input', () => setSearchQuery(searchInput.value));
}

// ─── Breadcrumbs ─────────────────────────────────────────────────────────────
function renderBreadcrumbs() {
    const crumbs = window.BROWSE_CTX.breadcrumbs || [];
    if (crumbs.length <= 1) return '';
    return `<div class="breadcrumb">
        ${crumbs.map((c, i) => {
            const isLast = i === crumbs.length - 1;
            return isLast
                ? `<span class="breadcrumb-item active">${escapeHtml(c.label)}</span>`
                : `<a class="breadcrumb-item" href="${escapeAttr(c.href || '/')}">${escapeHtml(c.label)}</a>
                   <i class="material-icons-round">chevron_right</i>`;
        }).join('')}
    </div>`;
}

// ─── Build API URL from BROWSE_CTX ───────────────────────────────────────────
function _buildApiUrl(page) {
    const ctx = window.BROWSE_CTX;
    const url = new URL('/api/jobs', window.location.origin);
    url.searchParams.set('page', page);
    url.searchParams.set('limit', JOBS_PER_PAGE);
    if (searchQuery) url.searchParams.set('search', searchQuery);
    if (ctx.category !== 'all') url.searchParams.set('category', ctx.category);

    if (ctx.view === 'series_list') {
        url.searchParams.set('group_by', 'series');
    } else if (ctx.view === 'seasons') {
        url.searchParams.set('group_by', 'season');
        url.searchParams.set('series_name', ctx.seriesName);
    } else if (ctx.view === 'episodes') {
        url.searchParams.set('series_name', ctx.seriesName);
        url.searchParams.set('season_number', ctx.seasonNumber === null ? 'null' : ctx.seasonNumber);
    }
    return url;
}

// ─── Job list ─────────────────────────────────────────────────────────────────
function loadJobs() {
    allJobs = [];
    videosContainer.innerHTML = `${renderBreadcrumbs()}<p class="no-results">Loading...</p>`;

    fetch(_buildApiUrl(1))
        .then(r => r.json())
        .then(data => {
            jobsPage = 1;
            allJobs = data.jobs || [];
            hasMoreJobs = !!data.has_more;
            loadMoreBtn.classList.toggle('visible', hasMoreJobs);
            renderJobs();
        })
        .catch(() => { videosContainer.innerHTML = '<p class="no-results">Could not load items.</p>'; });
}

function loadMoreJobs() {
    const nextPage = jobsPage + 1;
    fetch(_buildApiUrl(nextPage))
        .then(r => r.json())
        .then(data => {
            jobsPage = nextPage;
            const newJobs = data.jobs || [];
            allJobs = allJobs.concat(newJobs);
            hasMoreJobs = !!data.has_more;
            loadMoreBtn.classList.toggle('visible', hasMoreJobs);
            renderJobs();
        }).catch(() => {});
}

function renderJobs() {
    const ctx = window.BROWSE_CTX;
    const items = allJobs;

    const headerLabels = { all: 'All Videos', Film: 'Films', Series: 'Series', 'Anime Film': 'Anime Films', 'Anime TV': 'Anime TV' };
    let sectionTitle = headerLabels[ctx.category] || ctx.category;
    if (ctx.view === 'seasons') sectionTitle = ctx.seriesName || sectionTitle;

    const isCategoryGrid = ctx.view === 'grid' || ctx.view === 'series_list';
    let header = '';
    if (isCategoryGrid) {
        const count = items.length;
        const lower = sectionTitle.toLowerCase();
        header =
          `<div class="t-cat-head">
             <div class="eyebrow">Library · ${escapeHtml(sectionTitle)}</div>
             <h1>${count}${hasMoreJobs ? '+' : ''} ${escapeHtml(lower)}</h1>
             <div class="subtitle">Sorted by recently added.</div>
             <div class="t-fbar">
               <i class="material-icons-round">filter_list</i>
               <input class="t-chip" id="pageSearchInput" type="search" value="${escapeAttr(searchQuery)}" placeholder="Search this view" style="min-width:180px;text-align:left">
               ${['All','Unwatched','4K','HDR','HEVC','AV1','2024']
                  .map((c,i)=>`<button class="t-chip" aria-pressed="${i===0?'true':'false'}">${c}</button>`).join('')}
               <div style="flex:1"></div>
               <span style="color:var(--t-ink-3);font-size:13px">Sort</span>
               <button class="t-chip">Recently added</button>
             </div>
           </div>`;
    } else if (ctx.view === 'seasons') {
        header = renderSeriesDetailHeader(items);
    } else {
        header = renderBreadcrumbs() + `<h2 class="section-header">${escapeHtml(sectionTitle)}</h2>`;
    }

    if (items.length === 0) {
        videosContainer.innerHTML = `${header}<div class="no-results">
            <i class="material-icons-round">video_library</i>
            <p>No items found</p>
        </div>`;
        return;
    }

    let contentHtml = '';
    if (ctx.view === 'series_list') {
        contentHtml = `<div class="video-grid posters">${items.map(j => renderCard(j, 'series')).join('')}</div>`;
    } else if (ctx.view === 'seasons') {
        contentHtml = renderSeriesSeasonContent(items);
    } else if (ctx.view === 'episodes') {
        contentHtml = `<div class="t-episode-list" style="padding:0 56px 56px">${items.map(renderEpisodeRow).join('')}</div>`;
    } else {
        contentHtml = `<div class="video-grid">${items.map(j => renderCard(j, 'video')).join('')}</div>`;
    }

    videosContainer.innerHTML = header + contentHtml;

    // Wire filter chips (visual-only for now — toggle aria-pressed within the bar)
    const pageSearchInput = videosContainer.querySelector('#pageSearchInput');
    if (pageSearchInput) {
        pageSearchInput.addEventListener('input', () => setSearchQuery(pageSearchInput.value));
    }

    videosContainer.querySelectorAll('.t-fbar button.t-chip').forEach((chip, i, all) => {
        chip.addEventListener('click', () => {
            // sort chip is the last one — don't toggle the filter group
            if (i === all.length - 1) return;
            all.forEach((c, j) => {
                if (j === all.length - 1) return;
                c.setAttribute('aria-pressed', c === chip ? 'true' : 'false');
            });
        });
    });
}

function renderCard(j, type) {
    const ctx = window.BROWSE_CTX;
    const safeId = escapeAttr(j.job_id);
    const isAnimeCat = ctx.category === 'Anime TV' || ctx.category === 'Anime Film';
    const thumbSrc = (isAnimeCat ? j.poster_url : null) || (j.has_thumbnail ? `/thumbnail/${safeId}` : null);
    const gradient = jobIdToGradient(j.job_id);
    const thumbHtml = thumbSrc
        ? `<img class="thumb-img" src="${escapeAttr(thumbSrc)}" alt="" loading="lazy" onload="this.classList.add('loaded')" onerror="this.style.display='none';this.nextElementSibling.style.display='flex'">`
        : '';
    const placeholderStyle = thumbSrc ? 'display:none' : '';

    if (type === 'series') {
        const name = j.series_name || 'Unknown Series';
        const count = j.episode_count || 0;
        const catPath = CATEGORY_PATHS[ctx.category] || '/';
        const href = escapeAttr(catPath + '/' + slugify(name));
        return `<a class="video-card poster" href="${href}">
            <div class="thumb-wrap" style="background:${gradient}">
                ${thumbHtml}
                <div class="thumb-placeholder" style="${placeholderStyle}"><i class="material-icons-round">library_books</i></div>
                <div class="badge-count">${count}</div>
            </div>
            <div class="card-meta">
                <div class="card-title">${escapeHtml(name)}</div>
                <div class="card-subtitle"><span class="dot"></span>Series</div>
            </div>
        </a>`;
    }

    if (type === 'season') {
        const season = j.season_number;
        const count = j.episode_count || 0;
        const seasonLabel = season === null ? 'Specials' : `Season ${season}`;
        const catPath = CATEGORY_PATHS[ctx.category] || '/';
        const seasonPath = season === null ? '/specials' : `/s${season}`;
        const href = escapeAttr(catPath + '/' + (ctx.seriesSlug || slugify(ctx.seriesName || '')) + seasonPath);
        return `<a class="video-card poster" href="${href}">
            <div class="thumb-wrap" style="background:${gradient}">
                ${thumbHtml}
                <div class="thumb-placeholder" style="${placeholderStyle}"><i class="material-icons-round">folder</i></div>
                <div class="season-overlay">
                    <div class="season-label">${season === null ? '' : 'Season'}</div>
                    <div class="season-num">${season === null ? 'SP' : season}</div>
                </div>
                <div class="badge-count">${count}</div>
            </div>
            <div class="card-meta">
                <div class="card-title">${seasonLabel}</div>
                <div class="card-subtitle"><span class="dot"></span>${count} Episode${count !== 1 ? 's' : ''}</div>
            </div>
        </a>`;
    }

    // Default video card
    const dur = formatDuration(j.duration);
    const title = escapeHtml(cleanTitle(j.filename || j.job_id));
    const subtitleParts = [];
    if (j.media_type) subtitleParts.push(escapeHtml(j.media_type));
    if (j.season_number != null && j.episode_number != null) {
        subtitleParts.push(`S${String(j.season_number).padStart(2,'0')}E${String(j.episode_number).padStart(2,'0')}`);
    } else if (j.episode_number != null) {
        subtitleParts.push(`Ep ${j.episode_number}`);
    } else if (j.part_number != null) {
        subtitleParts.push(`Part ${j.part_number}`);
    }
    if (j.video_height) subtitleParts.push(`${j.video_height}p`);
    const subtitleHtml = subtitleParts.map((p, i) =>
        i === 0 ? p : `<span class="sep">&bull;</span> ${p}`
    ).join(' ');

    return `<a class="video-card" href="/watch/${safeId}" oncontextmenu="event.preventDefault();openEditModal('${safeId}');">
        <div class="thumb-wrap" style="background:${gradient}">
            ${thumbHtml}
            <div class="thumb-placeholder" style="${placeholderStyle}"><i class="material-icons-round">play_circle_filled</i></div>
            ${dur ? `<div class="thumb-duration">${dur}</div>` : ''}
            ${j.progress_pct && j.progress_pct > 0 && j.progress_pct < 100 
                ? `<div class="thumb-progress-bg"><div class="thumb-progress-fg" style="width:${j.progress_pct}%"></div></div>` 
                : ''}
        </div>
        <div class="card-meta">
            <div class="card-title">${title}</div>
            <div class="card-subtitle"><span class="dot"></span>${subtitleHtml}</div>
            <div class="player-actions" style="margin-top:0.75rem;display:flex;gap:0.5rem;">
                <button class="action-btn icon-only" title="Favorite" onclick="event.preventDefault();event.stopPropagation();toggleFavorite('${safeId}', this)"><i class="material-icons-round">favorite</i></button>
                <button class="action-btn icon-only" title="My List" onclick="event.preventDefault();event.stopPropagation();toggleWatchlist('${safeId}', this)"><i class="material-icons-round">bookmark_add</i></button>
                <button class="action-btn" onclick="event.preventDefault();openEditModal('${safeId}')">Edit</button>
                <button class="action-btn danger" onclick="event.preventDefault();deleteJob('${safeId}')">Delete</button>
            </div>
        </div>
    </a>`;
}

async function toggleFavorite(jobId, btn) {
    try {
        const data = await window.THLSUserData.toggleFavorite(jobId);
        if (btn) btn.classList.toggle('active', !!data.favorite);
    } catch (e) {
        alert(e.message || 'Favorite update failed');
    }
}

async function toggleWatchlist(jobId, btn) {
    try {
        const data = await window.THLSUserData.toggleWatchlist(jobId);
        if (btn) btn.classList.toggle('active', !!data.watchlisted);
    } catch (e) {
        alert(e.message || 'Watchlist update failed');
    }
}

// ─── Series detail (seasons view) ────────────────────────────────────────────
function renderSeriesDetailHeader(seasonItems) {
    const ctx = window.BROWSE_CTX;
    const sample = seasonItems.find(j => j.has_thumbnail) || seasonItems[0] || {};
    const seriesName = ctx.seriesName || cleanTitle(sample.filename || '');
    const totalEps = seasonItems.reduce((acc, s) => acc + (s.episode_count || 0), 0);
    const seasonCount = seasonItems.length;
    const grad = jobIdToGradient(sample.job_id || seriesName);
    const heroBg = sample.has_thumbnail
        ? `background-image:url('/thumbnail/${escapeAttr(sample.job_id)}');background-size:cover;background-position:center`
        : `background:${grad}`;
    const cat = window.BROWSE_CTX.category;
    const catRoot = cat === 'Anime TV' ? '/anime-tv' : '/series';
    const catLabel = cat === 'Anime TV' ? 'Anime TV' : 'Series';
    const resumeHref = sample.job_id ? `/watch/${escapeAttr(sample.job_id)}` : '#';

    return `
      <header class="t-hero t-series-hero">
        <div class="t-hero__art" style="${heroBg}"></div>
        <div class="t-hero__scrim"></div>
        <div class="t-hero__body">
          <div class="t-hero__eyebrow">
            <a href="${catRoot}" style="color:rgba(255,255,255,0.7)">${catLabel}</a>
            <i class="material-icons-round" style="font-size:12px;opacity:.6">chevron_right</i>
            <span style="color:#fff">${escapeHtml(seriesName)}</span>
          </div>
          <h1 class="t-hero__title">${escapeHtml(seriesName)}</h1>
          <div class="t-hero__meta">
            <span>${totalEps} episode${totalEps !== 1 ? 's' : ''}</span>
            <span class="t-dot"></span>
            <span>${seasonCount} season${seasonCount !== 1 ? 's' : ''}</span>
          </div>
          <div class="t-hero__actions">
            <a class="t-btn t-btn--primary" href="${resumeHref}">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M7 5.5v13l11-6.5z"/></svg>
              Play first episode
            </a>
          </div>
        </div>
      </header>`;
}

function renderSeriesSeasonContent(seasonItems) {
    const ctx = window.BROWSE_CTX;
    const detailKey = `${ctx.category || ''}\u0000${ctx.seriesName || ''}`;
    if (_seriesDetailKey !== detailKey) {
        _seriesDetailKey = detailKey;
        _seriesDetailSelected = null;
        _seriesDetailEpisodes = {};
    }
    const seriesSlug = ctx.seriesSlug || slugify(ctx.seriesName || '');
    const catRoot = ctx.category === 'Anime TV' ? '/anime-tv' : '/series';

    const sortedSeasons = seasonItems.slice().sort((a, b) => {
        const an = a.season_number == null ? 1e9 : a.season_number;
        const bn = b.season_number == null ? 1e9 : b.season_number;
        return an - bn;
    });
    if (_seriesDetailSelected == null) {
        _seriesDetailSelected = sortedSeasons[0]?.season_number ?? null;
    }

    const tabs = sortedSeasons.map(s => {
        const num = s.season_number;
        const label = num == null ? 'Specials' : `Season ${num}`;
        const isActive = num === _seriesDetailSelected;
        return `<button class="t-tab" data-season="${num == null ? 'specials' : num}"
                ${isActive ? 'aria-current="page"' : ''}>${label}</button>`;
    }).join('');

    setTimeout(() => loadSeasonEpisodes(_seriesDetailSelected), 0);

    return `
      <section class="t-section" style="padding-top:28px">
        <div class="t-series-meta-row">
          <h2>Episodes</h2>
          <div style="display:flex;gap:4px;flex-wrap:wrap" id="seasonTabs">${tabs}</div>
          <div style="flex:1"></div>
          <div style="color:var(--t-ink-3);font-size:13px" id="seasonSummary"></div>
        </div>
        <div class="t-episode-list" id="seasonEpisodeList">
          <div class="no-results" style="padding:32px"><p>Loading episodes…</p></div>
        </div>
      </section>`;
}

function loadSeasonEpisodes(seasonNumber) {
    const ctx = window.BROWSE_CTX;
    const tabs = document.getElementById('seasonTabs');
    if (tabs) {
        tabs.querySelectorAll('.t-tab').forEach(t => {
            const v = t.dataset.season;
            const matches = (seasonNumber == null && v === 'specials') || (String(seasonNumber) === v);
            if (matches) t.setAttribute('aria-current', 'page');
            else t.removeAttribute('aria-current');
            t.onclick = () => {
                const sn = v === 'specials' ? null : parseInt(v, 10);
                _seriesDetailSelected = sn;
                loadSeasonEpisodes(sn);
            };
        });
    }
    const list = document.getElementById('seasonEpisodeList');
    if (!list) return;

    const cacheKey = String(seasonNumber);
    if (_seriesDetailEpisodes[cacheKey]) {
        renderSeasonEpisodeList(_seriesDetailEpisodes[cacheKey], seasonNumber);
        return;
    }

    const url = new URL('/api/jobs', window.location.origin);
    url.searchParams.set('category', ctx.category);
    url.searchParams.set('series_name', ctx.seriesName);
    url.searchParams.set('season_number', seasonNumber === null ? 'null' : seasonNumber);
    url.searchParams.set('limit', '500');
    fetch(url)
        .then(r => r.json())
        .then(d => {
            const episodes = (d.jobs || []).slice().sort((a, b) =>
                (a.episode_number || 0) - (b.episode_number || 0));
            _seriesDetailEpisodes[cacheKey] = episodes;
            renderSeasonEpisodeList(episodes, seasonNumber);
        })
        .catch(() => {
            list.innerHTML = '<div class="no-results"><p>Could not load episodes.</p></div>';
        });
}

function renderSeasonEpisodeList(episodes, seasonNumber) {
    const list = document.getElementById('seasonEpisodeList');
    if (!list) return;
    if (!episodes.length) {
        list.innerHTML = '<div class="no-results"><p>No episodes in this season.</p></div>';
    } else {
        list.innerHTML = episodes.map(renderEpisodeRow).join('');
    }
    const summary = document.getElementById('seasonSummary');
    if (summary) {
        const label = seasonNumber == null ? 'Specials' : `Season ${seasonNumber}`;
        summary.textContent = `${label} · ${episodes.length} episode${episodes.length !== 1 ? 's' : ''}`;
    }
}

// ─── Episode list row (Series detail) ────────────────────────────────────────
function renderEpisodeRow(j) {
    const safeId = escapeAttr(j.job_id);
    const dur    = formatDuration(j.duration);
    const title  = escapeHtml(j.episode_title || cleanTitle(j.filename || j.job_id));
    const s = j.season_number, e = j.episode_number;
    const epCode = (s != null && e != null)
        ? `S${String(s).padStart(2,'0')}·E${String(e).padStart(2,'0')}`
        : (e != null ? `Ep ${e}` : '');
    const grad = jobIdToGradient(j.job_id);
    const thumb = j.has_thumbnail
        ? `<img class="thumb-img" src="/thumbnail/${safeId}" alt="" loading="lazy" onload="this.classList.add('loaded')">`
        : `<div class="thumb-placeholder"><i class="material-icons-round">play_circle_filled</i></div>`;
    const progressMap = (() => {
        try { return JSON.parse(localStorage.getItem('thls_progress_v1') || '{}') || {}; }
        catch { return {}; }
    })();
    const lp = progressMap[j.job_id];
    const progress = lp && lp.pct > 1 && lp.pct < 95
        ? `<div class="thumb-progress" style="--p:${lp.pct}%"></div>` : '';
    return `
      <a class="t-episode-row" href="/watch/${safeId}"
         oncontextmenu="event.preventDefault();openEditModal('${safeId}');">
        <div class="thumb-wrap" style="background:${grad}">
          ${thumb}
          ${dur ? `<div class="thumb-duration">${dur}</div>` : ''}
          ${progress}
        </div>
        <div style="min-width:0">
          <div class="t-episode-row__title">
            ${epCode ? `<div class="ep-code">${epCode}</div>` : ''}
            <div class="ep-name">${title}</div>
          </div>
          <div class="t-episode-row__desc">
            ${j.media_type ? escapeHtml(j.media_type) : ''}${j.video_height ? ` · ${j.video_height}p` : ''}${j.audio_count ? ` · ${j.audio_count} audio track${j.audio_count !== 1 ? 's' : ''}` : ''}
          </div>
        </div>
        <button class="t-iconbtn" title="More"
                onclick="event.preventDefault();event.stopPropagation();openEditModal('${safeId}');">
          <i class="material-icons-round">more_horiz</i>
        </button>
      </a>`;
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
        allJobs = allJobs.filter(j => j.job_id !== jobId);
        renderJobs();
    } catch (e) {
        alert(e.message);
    }
}

function closeEditModal() {
    document.getElementById('editModal').classList.remove('active');
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
    const job = allJobs.find(j => j.job_id === jobId);
    if (!job) return;

    document.getElementById('editJobId').value = job.job_id;
    document.getElementById('editTitle').value = cleanTitle(job.filename || job.job_id);
    document.getElementById('editCategory').value = getCategoryFromJob(job);
    document.getElementById('editSeriesName').value = job.series_name || '';
    document.getElementById('editSeasonNumber').value = job.season_number != null ? job.season_number : '';
    document.getElementById('editEpisodeNumber').value = job.episode_number != null ? job.episode_number : '';
    document.getElementById('editPartNumber').value = job.part_number != null ? job.part_number : '';

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

        const job = allJobs.find(j => j.job_id === jobId);
        if (job) {
            Object.assign(job, payload);
            if (payload.title) job.filename = payload.title;
            if (payload.part_number !== undefined) job.part_number = payload.part_number ? parseInt(payload.part_number) : null;
            if (payload.season_number !== undefined) job.season_number = payload.season_number ? parseInt(payload.season_number) : null;
            if (payload.episode_number !== undefined) job.episode_number = payload.episode_number ? parseInt(payload.episode_number) : null;

            if (!['Film Series'].includes(cat)) job.part_number = null;
            if (!['TV Series', 'Anime TV', 'Anime TV Series'].includes(cat)) {
                job.season_number = null;
                job.episode_number = null;
            }
            if (['Film', 'Anime Film'].includes(cat)) job.series_name = '';
        }
        closeEditModal();
        renderJobs();
    } catch (e) {
        alert(e.message);
    } finally {
        btn.disabled = false;
        btn.textContent = 'Save Changes';
    }
}

// ─── Init ─────────────────────────────────────────────────────────────────────
if (!window.__THLS_HOME_HANDLED__) loadJobs();
