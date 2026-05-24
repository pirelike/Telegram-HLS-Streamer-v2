// ============================================================
// THLS — browse-palette.js
// ⌘K / Ctrl+K command palette. Loaded on every page.
// ============================================================
(function () {
  let overlay = null, inputEl = null, listEl = null;
  let items = [];   // { type:'job'|'action', href?, go? }
  let activeIdx = -1;
  let debounce = null;

  function ensureDOM() {
    if (overlay) return;
    overlay = document.createElement('div');
    overlay.className = 'thls-pal-overlay';
    overlay.innerHTML =
      '<div class="thls-pal-box">' +
        '<div class="thls-pal-header">' +
          icon('<circle cx="11" cy="11" r="6.5"/><path d="m20 20-3.5-3.5"/>',
               'style="flex:0 0 auto;color:var(--t-ink-3)"') +
          '<input class="thls-pal-input" id="thls-pal-input"' +
               ' placeholder="Search films, series, episodes, settings…"' +
               ' autocomplete="off">' +
          '<kbd class="thls-kbd">esc</kbd>' +
        '</div>' +
        '<div class="thls-pal-list" id="thls-pal-list"></div>' +
        '<div class="thls-pal-footer">' +
          '<span><kbd class="thls-kbd">↑</kbd> <kbd class="thls-kbd">↓</kbd> Navigate</span>' +
          '<span><kbd class="thls-kbd">↵</kbd> Open</span>' +
          '<span style="margin-left:auto">THLS</span>' +
        '</div>' +
      '</div>';
    document.body.appendChild(overlay);

    inputEl = overlay.querySelector('#thls-pal-input');
    listEl  = overlay.querySelector('#thls-pal-list');

    overlay.addEventListener('click', e => { if (e.target === overlay) close(); });
    inputEl.addEventListener('input', () => { activeIdx = -1; scheduleSearch(inputEl.value.trim()); });
    inputEl.addEventListener('keydown', onKey);
  }

  function onKey(e) {
    if      (e.key === 'ArrowDown') { e.preventDefault(); moveActive(1); }
    else if (e.key === 'ArrowUp')   { e.preventDefault(); moveActive(-1); }
    else if (e.key === 'Enter')     { e.preventDefault(); activate(); }
  }

  function scheduleSearch(q) {
    clearTimeout(debounce);
    if (!q) { loadDefault(); return; }
    debounce = setTimeout(() => {
      fetch('/api/jobs?q=' + encodeURIComponent(q) + '&limit=6')
        .then(r => r.json()).then(d => render(d.jobs || [], q)).catch(() => render([], q));
    }, 130);
  }

  function loadDefault() {
    fetch('/api/jobs?limit=5')
      .then(r => r.json()).then(d => render(d.jobs || [], '')).catch(() => render([], ''));
  }

  function render(jobs, q) {
    items = [];
    const actions = quickActions(q);
    const sections = [];

    if (jobs.length) {
      const rows = jobs.map((j, i) => {
        const title = cleanTitle(j.filename || j.job_id);
        const sub   = [j.media_type,
                       j.video_height ? j.video_height + 'p' : null,
                       formatDur(j.duration)].filter(Boolean).join(' · ');
        const isSeries = j.is_series || (j.episode_count && j.episode_count > 0);
        const cat  = j.media_type === 'Anime TV' ? '/anime-tv' : '/series';
        const href = isSeries
          ? cat + '/' + slugify(j.series_name || title)
          : '/watch/' + j.job_id;
        const grad = gradient(j.job_id);
        const thumb = j.has_thumbnail
          ? '<img src="/thumbnail/' + esc(j.job_id) + '" style="width:100%;height:100%;object-fit:cover" loading="lazy">'
          : '';
        items.push({ type: 'job', href });
        return '<a href="' + esc(href) + '" class="thls-pal-row' + (i === 0 ? ' is-active' : '') +
               '" data-idx="' + (items.length - 1) + '">' +
               '<div class="thls-pal-thumb" style="background:' + grad + '">' + thumb + '</div>' +
               '<div style="flex:1;min-width:0">' +
               '<div style="font-size:14px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">' + escHtml(title) + '</div>' +
               '<div style="color:var(--t-ink-3);font-size:12px">' + escHtml(sub) + '</div>' +
               '</div>' +
               (i === 0 ? '<kbd class="thls-kbd">↵</kbd>' : '') +
               '</a>';
      }).join('');
      sections.push(group(q ? 'Best matches' : 'From your library', rows));
    }

    if (actions.length) {
      const rows = actions.map(a => {
        items.push({ type: 'action', go: a.go });
        return '<button class="thls-pal-row thls-pal-action" data-idx="' + (items.length - 1) + '">' +
               '<div class="thls-pal-icon">' + a.icon + '</div>' +
               '<div style="flex:1;font-size:14px">' + a.title + '</div>' +
               (a.hint ? '<kbd class="thls-kbd">' + a.hint + '</kbd>' : '') +
               '</button>';
      }).join('');
      sections.push(group('Quick actions', rows));
    }

    listEl.innerHTML = sections.length
      ? sections.join('')
      : '<div style="padding:32px;text-align:center;color:var(--t-ink-3);font-size:14px">No results</div>';

    listEl.querySelectorAll('.thls-pal-row').forEach(el => {
      el.addEventListener('click', e => {
        const idx = parseInt(el.dataset.idx, 10);
        if (items[idx]?.type === 'action') { e.preventDefault(); close(); items[idx].go(); }
        else close();
      });
    });
  }

  function group(label, rows) {
    return '<div style="padding:6px 4px 10px">' +
           '<div style="padding:6px 10px 4px;font-size:11px;font-weight:600;' +
                'color:var(--t-ink-3);letter-spacing:0.06em;text-transform:uppercase">' +
           label + '</div>' + rows + '</div>';
  }

  function moveActive(dir) {
    const rows = [...listEl.querySelectorAll('.thls-pal-row')];
    if (!rows.length) return;
    if (activeIdx < 0) activeIdx = dir > 0 ? 0 : rows.length - 1;
    else activeIdx = Math.max(0, Math.min(rows.length - 1, activeIdx + dir));
    rows.forEach((el, i) => el.classList.toggle('is-active', i === activeIdx));
    rows[activeIdx]?.scrollIntoView({ block: 'nearest' });
  }

  function activate() {
    const idx = activeIdx >= 0 ? activeIdx : 0;
    const item = items[idx];
    if (!item) return;
    if (item.type === 'action') { close(); item.go(); }
    else { close(); window.location.href = item.href; }
  }

  function quickActions(q) {
    const all = [
      { title: 'Upload a video', hint: '',
        icon: icon('<path d="M12 4v12"/><path d="m7 9 5-5 5 5"/><path d="M5 20h14"/>'),
        go: () => { window.location.href = '/upload'; } },
      { title: 'Open settings', hint: '',
        icon: icon('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h0a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v0a1.7 1.7 0 0 0 1.5 1H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>'),
        go: () => { window.location.href = '/settings'; } },
      { title: 'Toggle dark / light', hint: '⌘⇧L',
        icon: icon('<path d="M12 4l1.6 4.4L18 10l-4.4 1.6L12 16l-1.6-4.4L6 10l4.4-1.6z"/>' +
                   '<path d="M19 14l.8 2.2L22 17l-2.2.8L19 20l-.8-2.2L16 17l2.2-.8z"/>'),
        go: () => {
          const dark = document.documentElement.getAttribute('data-theme') !== 'dark';
          document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
          localStorage.setItem('hls_theme', dark ? 'dark' : 'light');
          if (typeof applyTheme === 'function') applyTheme(dark);
        } },
    ];
    if (!q) return all;
    const ql = q.toLowerCase();
    return all.filter(a => a.title.toLowerCase().includes(ql));
  }

  function open() {
    ensureDOM();
    overlay.style.display = 'grid';
    inputEl.value = '';
    activeIdx = -1;
    listEl.innerHTML = '';
    loadDefault();
    requestAnimationFrame(() => inputEl.focus());
  }

  function close() {
    if (overlay) overlay.style.display = 'none';
  }

  // ─── helpers ───────────────────────────────────────────────
  function icon(d, extra) {
    return '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"' +
           ' stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"' +
           (extra ? ' ' + extra : '') + '>' + d + '</svg>';
  }
  function esc(s) {
    return String(s == null ? '' : s)
      .replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
      .replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  }
  function escHtml(s) { return esc(s); }
  function slugify(s) { return String(s||'').toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-|-$/g,''); }
  function cleanTitle(n) {
    return String(n||'').replace(/\.[a-z0-9]{2,4}$/i,'').replace(/[._]+/g,' ').replace(/\s+/g,' ').trim();
  }
  function formatDur(s) {
    if (!s || s <= 0) return '';
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
    return h ? h + 'h ' + m + 'm' : m + 'm';
  }
  function gradient(id) {
    let h = 0;
    for (const c of String(id || '')) h = (h * 31 + c.charCodeAt(0)) >>> 0;
    const a = h % 360, b = (a + 60) % 360;
    return 'linear-gradient(135deg,hsl(' + a + ' 60% 30%),hsl(' + b + ' 50% 12%))';
  }

  // ─── global keyboard ───────────────────────────────────────
  document.addEventListener('keydown', e => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      overlay && overlay.style.display !== 'none' ? close() : open();
    } else if (e.key === 'Escape' && overlay && overlay.style.display !== 'none') {
      e.preventDefault(); close();
    }
  });

  // wire search trigger button
  document.addEventListener('DOMContentLoaded', () => {
    document.getElementById('searchTriggerBtn')?.addEventListener('click', open);
  });

  window.__thls_palette_open  = open;
  window.__thls_palette_close = close;
})();
