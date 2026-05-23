// ============================================================
// THLS — browse-home.js
// Runs only when BROWSE_CTX.view === "home".
// Renders: rotating hero with dots → section rows → stats strip.
// ============================================================

(function () {
  if (!window.BROWSE_CTX || window.BROWSE_CTX.view !== 'home') return;

  const heroMount   = document.getElementById('thlsHero');
  const container   = document.getElementById('videosContainer');
  const loadMoreBtn = document.getElementById('loadMoreBtn');
  if (!container) return;

  window.__THLS_HOME_HANDLED__ = true;
  window.__thls_update_status_pill = updateStatusPill;
  loadMoreBtn?.classList.remove('visible');

  // skeleton while loading
  if (heroMount) heroMount.innerHTML = '<div class="t-hero t-hero--skeleton"></div>';
  container.innerHTML = skeleton();

  const limit = 12;
  function fetchSlice(params) {
    const url = new URL('/api/jobs', location.origin);
    url.searchParams.set('page', 1);
    url.searchParams.set('limit', limit);
    for (const [k, v] of Object.entries(params || {})) url.searchParams.set(k, v);
    return fetch(url).then(r => r.json()).then(d => d.jobs || []);
  }

  Promise.all([
    fetchSlice({}),
    fetchSlice({ category: 'Film' }),
    fetchSlice({ category: 'Series',   group_by: 'series' }),
    fetchSlice({ category: 'Anime Film' }),
    fetchSlice({ category: 'Anime TV', group_by: 'series' }),
    fetchHealthWithProbe(),
  ]).then(async ([recent, films, series, animeFilms, animeTv, health]) => {
    renderHero(recent);
    await renderRows(recent, films, series, animeFilms, animeTv);
    renderStats(recent, health);
    updateStatusPill(health);
  }).catch(() => {
    if (heroMount) heroMount.innerHTML = '';
    container.innerHTML =
      '<div class="no-results"><i class="material-icons-round">error_outline</i><p>Could not load library.</p></div>';
  });

  // ─── Rotating hero ──────────────────────────────────────────
  function renderHero(items) {
    if (!heroMount) return;
    if (!items.length) { heroMount.innerHTML = ''; return; }

    // up to 3 spotlights: prefer items with thumbnails
    const withThumb    = items.filter(j => j.has_thumbnail);
    const withoutThumb = items.filter(j => !j.has_thumbnail);
    const spots        = [...withThumb, ...withoutThumb].slice(0, 3);

    const artDivs = spots.map((j, i) => {
      const bg = j.has_thumbnail
        ? 'background-image:url(\'/thumbnail/' + escAttr(j.job_id) + '\');background-size:cover;background-position:center'
        : 'background:' + gradient(j.job_id);
      return '<div class="t-hero__art" style="' + bg + ';opacity:' + (i === 0 ? 1 : 0) + ';transition:opacity 1.2s ease"></div>';
    }).join('');

    const dotsHtml = spots.length > 1
      ? '<div class="t-hero__dots">' +
          spots.map((_, i) =>
            '<button class="t-hero__dot" aria-label="Spotlight ' + (i + 1) + '"' +
            (i === 0 ? ' aria-current="true"' : '') + '></button>'
          ).join('') +
        '</div>'
      : '';

    heroMount.innerHTML =
      '<header class="t-hero">' +
        artDivs +
        '<div class="t-hero__scrim"></div>' +
        '<div class="t-hero__body" id="thls-hero-body"></div>' +
        dotsHtml +
      '</header>';

    let current = 0;
    const artEls = heroMount.querySelectorAll('.t-hero__art');
    const bodyEl = heroMount.querySelector('#thls-hero-body');
    const dotEls = heroMount.querySelectorAll('.t-hero__dot');

    function showSlide(idx) {
      artEls.forEach((el, i) => { el.style.opacity = i === idx ? '1' : '0'; });
      dotEls.forEach((el, i) => { el.setAttribute('aria-current', i === idx ? 'true' : 'false'); });
      // retrigger animation
      bodyEl.style.animation = 'none';
      void bodyEl.offsetHeight;
      bodyEl.style.animation = 'thlsHeroIn .7s cubic-bezier(.2,.7,.3,1) both';
      bodyEl.innerHTML = heroBodyHtml(spots[idx]);
      current = idx;
    }

    let timer = null;
    function startTimer() {
      if (spots.length < 2) return;
      clearInterval(timer);
      timer = setInterval(() => showSlide((current + 1) % spots.length), 9000);
    }

    dotEls.forEach((dot, i) => dot.addEventListener('click', () => {
      clearInterval(timer);
      showSlide(i);
      startTimer();
    }));

    showSlide(0);
    startTimer();
  }

  function heroBodyHtml(j) {
    const title  = esc(cleanTitle(j.filename || j.job_id));
    const type   = j.media_type || '';
    const height = j.video_height ? j.video_height + 'p' : '';
    const dur    = fmtDur(j.duration);
    const metas  = [type, height, dur].filter(Boolean);
    const metaHtml = metas.map((m, i) =>
      (i > 0 ? '<span style="width:3px;height:3px;border-radius:999px;background:currentColor;opacity:.5;display:inline-block;vertical-align:middle;margin:0 2px"></span>' : '') +
      '<span>' + esc(m) + '</span>'
    ).join('');
    return '<div class="t-hero__eyebrow">' +
             '<span class="t-hero__chip">Featured</span>' +
             (type ? '<span>' + esc(type) + '</span>' : '') +
           '</div>' +
           '<h1 class="t-hero__title">' + title + '</h1>' +
           (metas.length ? '<div class="t-hero__meta">' + metaHtml + '</div>' : '') +
           '<div class="t-hero__actions">' +
             '<a class="t-btn t-btn--primary" href="/watch/' + escAttr(j.job_id) + '">' +
               '<svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" stroke="none">' +
               '<path d="M7 5.5v13l11-6.5z"/></svg> Play' +
             '</a>' +
             '<button class="t-btn t-btn--ghost" type="button">' +
               '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">' +
               '<path d="M12 5v14"/><path d="M5 12h14"/></svg> Watchlist' +
             '</button>' +
           '</div>';
  }

  // ─── Rows ───────────────────────────────────────────────────
  async function renderRows(recent, films, series, animeFilms, animeTv) {
    // Fetch server-side playback progress.
    let serverProgress = {};
    try {
      let clientId = localStorage.getItem('thls_client_id_v1');
      if (clientId) {
        const resp = await fetch('/api/playback/progress?client_id=' + encodeURIComponent(clientId));
        if (resp.ok) {
          const data = await resp.json();
          for (const p of (data.progress || [])) {
            serverProgress[p.job_id] = p;
          }
        }
      }
    } catch {}

    // Merge persisted localStorage progress with server progress.
    const localProgress = (function () {
      try { return JSON.parse(localStorage.getItem('thls_progress_v1') || '{}') || {}; }
      catch { return {}; }
    })();
    const annotated = recent.map(j => {
      const sp = serverProgress[j.job_id];
      const lp = localProgress[j.job_id];
      // Server progress takes precedence, localStorage is fallback.
      if (sp && sp.progress_pct > 1 && sp.progress_pct < 95) {
        return Object.assign({}, j, { progress_pct: sp.progress_pct, resume_seconds: Math.floor(sp.position_seconds) });
      }
      if (lp && lp.pct > 1 && lp.pct < 95) {
        return Object.assign({}, j, { progress_pct: lp.pct, resume_seconds: lp.seconds });
      }
      return j;
    });

    const cw = annotated
      .filter(j => j.progress_pct && j.progress_pct > 0 && j.progress_pct < 95)
      .sort((a, b) => {
        const sa = serverProgress[a.job_id];
        const sb = serverProgress[b.job_id];
        const la = localProgress[a.job_id];
        const lb = localProgress[b.job_id];
        const tsA = (sa && Date.parse(sa.updated_at)) || (la?.ts) || 0;
        const tsB = (sb && Date.parse(sb.updated_at)) || (lb?.ts) || 0;
        return tsB - tsA;
      });
    const anime = [...animeFilms, ...animeTv];
    const rows  = [];

    if (cw.length)     rows.push(rowHtml('Continue Watching', cw,     'video'));
    if (recent.length) rows.push(rowHtml('Recently Added',    annotated, 'video'));
    if (films.length)  rows.push(rowHtml('Films',  films,  'video',  '/films'));
    if (series.length) rows.push(rowHtml('Series', series, 'series', '/series'));
    if (anime.length)  rows.push(rowHtml('Anime',  anime,  'mixed'));

    container.innerHTML = rows.length
      ? rows.join('')
      : '<div class="no-results"><i class="material-icons-round">movie</i><p>No videos yet — <a href="/upload">upload one</a>!</p></div>';

    wireRows();
  }

  function rowHtml(title, items, type, seeHref) {
    const cards = items.map(j => cardHtml(j, type)).join('');
    return '<section class="t-section">' +
             '<div class="t-section-head">' +
               '<div><h2 class="t-section-title">' + esc(title) + '</h2></div>' +
               (seeHref ? '<a class="t-section-see" href="' + seeHref + '">See all ›</a>' : '') +
             '</div>' +
             '<div class="t-row">' + cards + '</div>' +
           '</section>';
  }

  function cardHtml(j, type) {
    const isSeries = type === 'series' || (type === 'mixed' && (j.episode_count || j.series_name));
    if (isSeries) return seriesCardHtml(j);

    const safeId = escAttr(j.job_id);
    const dur    = fmtDur(j.duration);
    const title  = esc(cleanTitle(j.filename || j.job_id));
    const sub    = [j.media_type,
                    j.season_number != null && j.episode_number != null
                      ? 'S' + pad(j.season_number) + 'E' + pad(j.episode_number) : null,
                    j.video_height ? j.video_height + 'p' : null,
                   ].filter(Boolean).map(esc);
    const subHtml = sub.map((s, i) => (i === 0 ? s : '<span class="sep">·</span> ' + s)).join(' ');
    const grad    = gradient(j.job_id);
    const thumb   = j.has_thumbnail
      ? '<img class="thumb-img" src="/thumbnail/' + safeId + '" alt="" loading="lazy" onload="this.classList.add(\'loaded\')">'
      : '<div class="thumb-placeholder"><i class="material-icons-round">play_circle_filled</i></div>';
    const progress = j.progress_pct && j.progress_pct > 0 && j.progress_pct < 100
      ? '<div style="position:absolute;left:0;right:0;bottom:0;height:3px;background:rgba(255,255,255,.18)">' +
          '<div style="width:' + j.progress_pct + '%;height:100%;background:var(--t-accent)"></div>' +
        '</div>'
      : '';
    return '<a class="video-card" href="/watch/' + safeId + '"' +
              ' oncontextmenu="event.preventDefault();window.openEditModal&&openEditModal(\'' + safeId + '\')">' +
             '<div class="thumb-wrap" style="background:' + grad + '">' +
               thumb +
               (dur ? '<div class="thumb-duration">' + dur + '</div>' : '') +
               progress +
             '</div>' +
             '<div class="card-meta">' +
               '<div class="card-title">' + title + '</div>' +
               '<div class="card-subtitle">' + subHtml + '</div>' +
             '</div>' +
           '</a>';
  }

  function seriesCardHtml(j) {
    const name  = j.series_name || cleanTitle(j.filename || j.job_id);
    const count = j.episode_count || 0;
    const cat   = j.media_type === 'Anime TV' ? '/anime-tv' : '/series';
    const href  = cat + '/' + slugify(name);
    const grad  = gradient(j.job_id || name);
    const thumb = j.has_thumbnail
      ? '<img class="thumb-img" src="/thumbnail/' + escAttr(j.job_id) + '" alt="" loading="lazy" onload="this.classList.add(\'loaded\')">'
      : '<div class="thumb-placeholder"><i class="material-icons-round">library_books</i></div>';
    return '<a class="video-card" href="' + href + '">' +
             '<div class="thumb-wrap" style="background:' + grad + '">' +
               thumb +
               '<div class="badge-count">' + count + '</div>' +
             '</div>' +
             '<div class="card-meta">' +
               '<div class="card-title">' + esc(name) + '</div>' +
               '<div class="card-subtitle">' + count + ' episode' + (count !== 1 ? 's' : '') + '</div>' +
             '</div>' +
           '</a>';
  }

  function wireRows() {
    container.querySelectorAll('.t-row').forEach(row => {
      row.addEventListener('wheel', e => {
        if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
        if (e.shiftKey) { row.scrollLeft += e.deltaY; e.preventDefault(); }
      }, { passive: false });
    });
  }

  // ─── Stats strip ────────────────────────────────────────────
  function renderStats(recent, health) {
    const q      = health?.queue;
    const bots   = health?.bots;
    const active = q ? (q.active || 0) : 0;
    const pending = q ? (q.pending || 0) : 0;

    const queueVal   = active + pending > 0
      ? active + ' processing' + (pending > 0 ? ' · ' + pending + ' queued' : '')
      : 'Idle';
    const botsOk    = bots?.healthy ?? '—';
    const botsCfg   = bots?.configured ?? 0;
    const botsVal   = botsCfg > 0 ? botsOk + ' / ' + botsCfg + ' online' : '—';
    const botsUp    = botsCfg > 0 && botsOk === botsCfg;

    const strip =
      '<section class="t-section" style="padding-bottom:56px">' +
        '<div class="thls-stats">' +
          statEl('Library',       recent.length + '+ titles',  '', false) +
          statEl('Queue',         queueVal,                     '', false) +
          statEl('Telegram Bots', botsVal,                      '', botsUp) +
          statEl('Health',        health ? (health.status === 'ok' ? 'OK' : 'Degraded') : '—', '', health?.status === 'ok') +
        '</div>' +
      '</section>';

    container.insertAdjacentHTML('beforeend', strip);
  }

  function statEl(label, value, delta, up) {
    return '<div class="thls-stat">' +
             '<div class="thls-stat__label">' + esc(label) + '</div>' +
             '<div class="thls-stat__value">' + esc(value) + '</div>' +
             (delta ? '<div class="thls-stat__delta' + (up ? ' thls-stat__delta--up' : '') + '">' + esc(delta) + '</div>' : '') +
           '</div>';
  }

  // ─── Health (with first-load bot probe) ──────────────────────
  // /health returns cached probe results. If no probe has run yet, trigger
  // one so the navbar pill and stats reflect real bot reachability.
  function fetchHealthWithProbe() {
    return fetch('/health').then(r => r.json()).catch(() => null)
      .then(health => {
        if (!health) return null;
        const cfg = health.bots?.configured || 0;
        const probed = (health.bots?.last_probe || []).length;
        if (cfg > 0 && probed === 0) {
          // Hit the probe endpoint, then re-fetch /health to pick up the cache.
          return fetch('/api/bots/health', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: '{}',
          }).then(() => fetch('/health').then(r => r.json())).catch(() => health);
        }
        return health;
      });
  }

  // ─── Navbar live status pill ─────────────────────────────────
  function updateStatusPill(health) {
    const pill   = document.getElementById('thls-status-pill');
    const textEl = document.getElementById('thls-status-text');
    if (!pill || !textEl) return;
    const active   = health?.queue?.active || 0;
    const bots     = health?.bots || {};
    const cfg      = bots.configured || 0;
    const ok       = bots.healthy || 0;
    const probes   = bots.last_probe || [];
    if (active > 0) {
      pill.classList.add('t-livepill--processing');
      textEl.textContent = 'Processing · ' + active + ' job' + (active > 1 ? 's' : '');
    } else if (cfg === 0) {
      pill.classList.remove('t-livepill--processing');
      textEl.textContent = 'No bots configured';
    } else if (probes.length === 0) {
      pill.classList.remove('t-livepill--processing');
      textEl.textContent = 'Bots: not probed';
    } else if (ok === cfg) {
      pill.classList.remove('t-livepill--processing');
      textEl.textContent = 'All bots healthy';
    } else {
      pill.classList.add('t-livepill--processing');
      textEl.textContent = ok + ' / ' + cfg + ' bots online';
    }
    pill.style.display = '';
  }

  // ─── Skeleton ────────────────────────────────────────────────
  function skeleton() {
    const row = '<div class="t-row" style="margin-top:8px">' +
      Array.from({ length: 5 }).map(() =>
        '<div style="border-radius:14px;background:var(--t-surface-lo);aspect-ratio:16/9;' +
        'animation:thlsPulse 1.4s ease-in-out infinite"></div>'
      ).join('') + '</div>';
    return '<section class="t-section">' + row + '</section>' +
           '<section class="t-section">' + row + '</section>' +
           '<style>@keyframes thlsPulse{0%,100%{opacity:.5}50%{opacity:.9}}</style>';
  }

  // ─── Helpers (module-scoped, not global) ────────────────────
  function esc(s) {
    return String(s == null ? '' : s)
      .replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
      .replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  }
  function escAttr(s) { return esc(s); }
  function pad(n) { return String(n).padStart(2,'0'); }
  function slugify(s) { return String(s||'').toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-|-$/g,''); }
  function cleanTitle(n) {
    return String(n||'').replace(/\.[a-z0-9]{2,4}$/i,'').replace(/[._]+/g,' ').replace(/\s+/g,' ').trim();
  }
  function fmtDur(seconds) {
    if (!Number.isFinite(seconds) || seconds <= 0) return '';
    const h = Math.floor(seconds / 3600), m = Math.floor((seconds % 3600) / 60);
    return h ? h + 'h ' + m + 'm' : m + 'm';
  }
  function gradient(seed) {
    let h = 0;
    for (const c of String(seed || '')) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    const a = h % 360, b = (a + 60) % 360;
    return 'linear-gradient(135deg,hsl(' + a + ' 60% 30%),hsl(' + b + ' 50% 12%))';
  }
})();

// ─── Global helpers used by browse.js on non-home pages ─────────────────────
function escapeHtml(s) {
  return String(s == null ? '' : s)
    .replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
    .replace(/"/g,'&quot;').replace(/'/g,'&#39;');
}
function escapeAttr(s) { return escapeHtml(s); }
function pad(n) { return String(n).padStart(2,'0'); }
function slugify(s) { return String(s||'').toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-|-$/g,''); }
function formatDuration(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return '';
  const h = Math.floor(seconds / 3600), m = Math.floor((seconds % 3600) / 60);
  return h ? h + 'h ' + m + 'm' : m + 'm';
}
function cleanTitle(name) {
  return String(name||'').replace(/\.[a-z0-9]{2,4}$/i,'').replace(/[._]+/g,' ').replace(/\s+/g,' ').trim();
}
function jobIdToGradient(seed) {
  let h = 0;
  for (const c of String(seed||'')) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const a = h % 360, b = (a + 60) % 360;
  return 'linear-gradient(135deg,hsl(' + a + ' 60% 30%),hsl(' + b + ' 50% 12%))';
}
