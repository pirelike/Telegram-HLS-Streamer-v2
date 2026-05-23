// ============================================================
// THLS — browse-home.js
// ------------------------------------------------------------
// Save as: static/browse-home.js
// Loaded BEFORE browse.js by frontend.rs.
//
// What it does:
//   • On Home (BROWSE_CTX.view === "home"), short-circuits the
//     default browse.js render: it fetches a few category slices
//     from /api/jobs and renders a rotating hero + horizontal
//     rows (Continue Watching, Recently Added, Films, Series,
//     Anime). It then prevents browse.js from running its
//     initial loadJobs() so we don't double-render.
//   • On any other view, does nothing — browse.js renders the
//     grid as it always did.
// ============================================================

(function () {
  if (!window.BROWSE_CTX || window.BROWSE_CTX.view !== "home") return;

  const heroMount = document.getElementById("thlsHero");
  const container = document.getElementById("videosContainer");
  const loadMoreBtn = document.getElementById("loadMoreBtn");
  if (!container) return;

  // Stop browse.js from also running its loadJobs() on home.
  // It binds a search-input handler and an initial loadJobs() at the
  // bottom of the file; we hide that path by swapping BROWSE_CTX.view
  // briefly during init AFTER we've rendered, but the cleaner option
  // used here is to set a flag and short-circuit loadJobs.
  window.__THLS_HOME_HANDLED__ = true;

  loadMoreBtn?.classList.remove("visible");

  const limit = 12;
  const fetchSlice = (params) => {
    const url = new URL("/api/jobs", window.location.origin);
    url.searchParams.set("page", 1);
    url.searchParams.set("limit", limit);
    for (const [k, v] of Object.entries(params || {})) url.searchParams.set(k, v);
    return fetch(url).then((r) => r.json()).then((d) => d.jobs || []);
  };

  // Skeleton while loading
  container.innerHTML = skeleton();

  Promise.all([
    fetchSlice({}),                                                  // recent (any)
    fetchSlice({ category: "Film" }),
    fetchSlice({ category: "Series", group_by: "series" }),
    fetchSlice({ category: "Anime Film" }),
    fetchSlice({ category: "Anime TV", group_by: "series" }),
  ])
    .then(([recent, films, series, animeFilms, animeTv]) => {
      const featured = pickFeatured(recent);
      renderHero(featured);
      const rows = [];
      const cw = recent.filter((j) => j.progress_pct && j.progress_pct > 0 && j.progress_pct < 95);
      if (cw.length) rows.push(rowHtml("Continue Watching", cw, "video"));
      if (recent.length) rows.push(rowHtml("Recently Added", recent, "video"));
      if (films.length) rows.push(rowHtml("Films", films, "video", "/films"));
      if (series.length) rows.push(rowHtml("Series", series, "series", "/series"));
      const anime = [...animeFilms, ...animeTv];
      if (anime.length) rows.push(rowHtml("Anime", anime, "mixed"));
      container.innerHTML = rows.join("");
      wireRows(container);
    })
    .catch(() => {
      container.innerHTML =
        '<div class="no-results"><i class="material-icons-round">error_outline</i><p>Could not load library.</p></div>';
    });

  // ─── Hero ──────────────────────────────────────────────
  function pickFeatured(items) {
    // Prefer a film with a thumbnail; fall back to the first item.
    const withArt = items.find((i) => i.has_thumbnail);
    return withArt || items[0] || null;
  }

  function renderHero(j) {
    if (!j || !heroMount) return;
    const art = j.has_thumbnail
      ? `<div class="t-hero__art" style="background-image:url('/thumbnail/${j.job_id}')"></div>`
      : `<div class="t-hero__art" style="background:${jobIdToGradient(j.job_id)}"></div>`;
    const title = escapeHtml(cleanTitle(j.filename || j.job_id));
    const meta = [j.media_type, j.video_height ? j.video_height + "p" : null, formatDuration(j.duration)]
      .filter(Boolean).map(escapeHtml).join(" · ");
    heroMount.innerHTML = `
      <header class="t-hero">
        ${art}
        <div class="t-hero__scrim"></div>
        <div class="t-hero__body">
          <div class="t-hero__eyebrow">
            <span class="t-hero__chip">Featured</span>
            <span>${meta || "From your library"}</span>
          </div>
          <h1 class="t-hero__title">${title}</h1>
          <div class="t-hero__actions">
            <a class="t-btn t-btn--primary" href="/watch/${encodeURIComponent(j.job_id)}">
              <span class="material-icons-round" style="font-size:18px;">play_arrow</span> Play
            </a>
            <button class="t-btn t-btn--ghost" type="button">
              <span class="material-icons-round" style="font-size:18px;">add</span> Watchlist
            </button>
          </div>
        </div>
      </header>`;
  }

  // ─── Rows ──────────────────────────────────────────────
  function rowHtml(title, items, type, seeHref) {
    return `
      <section class="t-section">
        <div class="t-section-head">
          <div>
            <h2 class="t-section-title">${escapeHtml(title)}</h2>
          </div>
          ${seeHref ? `<a class="t-section-see" href="${seeHref}">See all ›</a>` : ""}
        </div>
        <div class="t-row">${items.map((j) => cardHtml(j, type)).join("")}</div>
      </section>`;
  }

  function cardHtml(j, type) {
    const isSeries = type === "series" || (type === "mixed" && (j.episode_count || j.series_name));
    if (isSeries) return seriesCardHtml(j);
    const safeId = escapeAttr(j.job_id);
    const thumbHref = j.has_thumbnail ? `/thumbnail/${safeId}` : null;
    const grad = jobIdToGradient(j.job_id);
    const dur = formatDuration(j.duration);
    const title = escapeHtml(cleanTitle(j.filename || j.job_id));
    const sub = [
      j.media_type,
      j.season_number != null && j.episode_number != null
        ? `S${pad(j.season_number)}E${pad(j.episode_number)}`
        : null,
      j.video_height ? `${j.video_height}p` : null,
    ].filter(Boolean).map(escapeHtml);
    const subHtml = sub.map((s, i) => (i === 0 ? s : `<span class="sep">·</span> ${s}`)).join(" ");
    const progress =
      j.progress_pct && j.progress_pct > 0 && j.progress_pct < 100
        ? `<div class="t-thumb__progress" style="position:absolute;left:0;right:0;bottom:0;height:3px;background:rgba(255,255,255,0.18);">
             <div style="width:${j.progress_pct}%;height:100%;background:var(--t-accent);"></div>
           </div>`
        : "";
    return `
      <a class="video-card" href="/watch/${safeId}" oncontextmenu="event.preventDefault();window.openEditModal&&openEditModal('${safeId}');">
        <div class="thumb-wrap" style="background:${grad}">
          ${
            thumbHref
              ? `<img class="thumb-img" src="${thumbHref}" alt="" loading="lazy" onload="this.classList.add('loaded')">`
              : `<div class="thumb-placeholder"><i class="material-icons-round">play_circle_filled</i></div>`
          }
          ${dur ? `<div class="thumb-duration">${dur}</div>` : ""}
          ${progress}
        </div>
        <div class="card-meta">
          <div class="card-title">${title}</div>
          <div class="card-subtitle">${subHtml || ""}</div>
        </div>
      </a>`;
  }

  function seriesCardHtml(j) {
    const name = j.series_name || cleanTitle(j.filename || j.job_id);
    const count = j.episode_count || 0;
    const cat = j.media_type === "Anime TV" ? "/anime-tv" : "/series";
    const href = `${cat}/${slugify(name)}`;
    const grad = jobIdToGradient(j.job_id || name);
    const thumbHref = j.has_thumbnail ? `/thumbnail/${escapeAttr(j.job_id)}` : null;
    return `
      <a class="video-card" href="${href}">
        <div class="thumb-wrap" style="background:${grad}">
          ${
            thumbHref
              ? `<img class="thumb-img" src="${thumbHref}" alt="" loading="lazy" onload="this.classList.add('loaded')">`
              : `<div class="thumb-placeholder"><i class="material-icons-round">library_books</i></div>`
          }
          <div class="badge-count">${count}</div>
        </div>
        <div class="card-meta">
          <div class="card-title">${escapeHtml(name)}</div>
          <div class="card-subtitle">${count} episode${count !== 1 ? "s" : ""}</div>
        </div>
      </a>`;
  }

  function skeleton() {
    const sk = (h) => `<div class="t-row" style="margin-top:8px;">
      ${Array.from({ length: 5 }).map(() => `
        <div style="border-radius:14px;background:var(--t-surface-lo);aspect-ratio:16/9;
                    animation:thlsPulse 1.4s ease-in-out infinite;"></div>`).join("")}
    </div>`;
    return `<section class="t-section">${sk()}</section>
            <section class="t-section">${sk()}</section>
            <style>@keyframes thlsPulse { 0%,100%{opacity:.6} 50%{opacity:1} }</style>`;
  }

  function wireRows(root) {
    // Horizontal scroll on shift+wheel (mouse users)
    root.querySelectorAll(".t-row").forEach((row) => {
      row.addEventListener("wheel", (e) => {
        if (e.deltaY === 0 || e.shiftKey) return;
        // let vertical scroll pass through, but allow row to scroll horizontally with shift+wheel
      });
    });
  }
})();

// ─── shared helpers (duplicated here so browse-home.js stands alone) ──
function escapeHtml(s) {
  return String(s == null ? "" : s)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}
function escapeAttr(s) { return escapeHtml(s); }
function pad(n) { return String(n).padStart(2, "0"); }
function slugify(s) {
  return String(s).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}
function formatDuration(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
}
function cleanTitle(name) {
  return String(name || "")
    .replace(/\.[a-z0-9]{2,4}$/i, "")
    .replace(/[._]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}
function jobIdToGradient(seed) {
  // Deterministic warm gradient per id
  let h = 0;
  for (const c of String(seed || "")) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const a = h % 360;
  const b = (a + 60) % 360;
  return `linear-gradient(135deg, hsl(${a} 60% 30%), hsl(${b} 50% 12%))`;
}
