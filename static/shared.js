// ─── Category → DB mapping (used by upload and edit modal) ───────────────────
const CATEGORY_DB = {
    'Film':           { media_type: 'Film',     is_series: 0 },
    'Film Series':    { media_type: 'Film',     is_series: 1 },
    'TV Series':      { media_type: 'Series',   is_series: 1 },
    'Anime Film':     { media_type: 'Anime Film', is_series: 0 },
    'Anime TV':       { media_type: 'Anime TV', is_series: 0 },
    'Anime TV Series':{ media_type: 'Anime TV', is_series: 1 },
};

// ─── Category path map (used by browse.js for URL construction) ───────────────
const CATEGORY_PATHS = {
    'all': '/', 'Film': '/films', 'Series': '/series',
    'Anime Film': '/anime-films', 'Anime TV': '/anime-tv',
};

// ─── Utilities ────────────────────────────────────────────────────────────────
function slugify(text) {
    return String(text || '').toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/-+/g, '-')
        .replace(/^-|-$/g, '');
}

function cleanTitle(filename) {
    if (!filename) return 'Untitled';
    let name = filename.replace(/^[0-9a-f]{16}_/i, '');
    name = name.replace(/\.[^.]+$/, '');
    name = name.replace(/[_.]/g, ' ');
    name = name.replace(/\s+/g, ' ').trim();
    return name;
}

function jobIdToGradient(jobId) {
    let hash = 0;
    const s = jobId || '';
    for (let i = 0; i < s.length; i++) {
        hash = Math.imul(31, hash) + s.charCodeAt(i) | 0;
    }
    const h1 = Math.abs(hash) % 360;
    const h2 = (h1 + 45) % 360;
    return `linear-gradient(145deg, hsl(${h1},40%,22%) 0%, hsl(${h2},50%,14%) 100%)`;
}

function escapeHtml(str) {
    const d = document.createElement('div');
    d.appendChild(document.createTextNode(String(str || '')));
    return d.innerHTML;
}

function escapeAttr(str) {
    return String(str || '').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function formatBytes(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes/1024).toFixed(1) + ' KB';
    if (bytes < 1073741824) return (bytes/1048576).toFixed(1) + ' MB';
    return (bytes/1073741824).toFixed(2) + ' GB';
}

function formatTime(seconds) {
    if (!isFinite(seconds) || seconds < 0) return '?';
    if (seconds < 60) return Math.round(seconds) + 's';
    if (seconds < 3600) return Math.round(seconds/60) + 'm ' + Math.round(seconds%60) + 's';
    return Math.floor(seconds/3600) + 'h ' + Math.round((seconds%3600)/60) + 'm';
}

function formatDuration(seconds) {
    if (!seconds || seconds <= 0) return '';
    const h = Math.floor(seconds/3600), m = Math.floor((seconds%3600)/60), s = Math.round(seconds%60);
    if (h > 0) return `${h}:${String(m).padStart(2,'0')}:${String(s).padStart(2,'0')}`;
    return `${m}:${String(s).padStart(2,'0')}`;
}

// ─── Theme ────────────────────────────────────────────────────────────────────
const THEME_KEY = 'hls_theme';
const themeToggleBtn = document.getElementById('themeToggleBtn');

function applyTheme(dark) {
    document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
    themeToggleBtn.innerHTML = dark
        ? '<i class="material-icons-round">light_mode</i>'
        : '<i class="material-icons-round">dark_mode</i>';
    themeToggleBtn.title = dark ? 'Switch to light mode' : 'Switch to dark mode';
}

function initTheme() {
    const saved = localStorage.getItem(THEME_KEY);
    const prefersDark = saved ? saved === 'dark' : window.matchMedia('(prefers-color-scheme: dark)').matches;
    applyTheme(prefersDark);
}

themeToggleBtn.addEventListener('click', () => {
    const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
    applyTheme(!isDark);
    localStorage.setItem(THEME_KEY, !isDark ? 'dark' : 'light');
});

initTheme();

// ─── Sidebar toggle (all pages) ──────────────────────────────────────────────
let sidebarOpen = window.innerWidth > 1024;

function updateSidebar() {
    const sidebar = document.getElementById('sidebar');
    const mainEl = document.getElementById('mainContent');
    if (!sidebar) return;
    if (window.innerWidth <= 1024) {
        sidebar.classList.toggle('open', sidebarOpen);
        sidebar.classList.remove('collapsed');
        if (mainEl) mainEl.classList.add('sidebar-collapsed');
    } else {
        sidebar.classList.remove('open');
        sidebar.classList.toggle('collapsed', !sidebarOpen);
        if (mainEl) mainEl.classList.toggle('sidebar-collapsed', !sidebarOpen);
    }
}

(function () {
    const btn = document.getElementById('hamburgerBtn');
    if (!btn) return;
    btn.addEventListener('click', () => {
        sidebarOpen = !sidebarOpen;
        updateSidebar();
    });
    window.addEventListener('resize', updateSidebar);
    updateSidebar();
})();

// ─── Global active-jobs panel ────────────────────────────────────────────────
(function () {
    const panel = document.getElementById('jobsPanel');
    const list = document.getElementById('jobsPanelList');
    const pill = document.getElementById('thls-status-pill');
    const pillText = document.getElementById('thls-status-text');
    if (!panel || !list || !pill || !pillText) return;

    let open = false;
    let pollTimer = null;

    window.__thls_toggle_jobs_panel = function () {
        open = !open;
        panel.classList.toggle('hidden', !open);
        if (open) { pollActiveJobs(); startPolling(); }
        else { stopPolling(); }
    };

    document.addEventListener('click', function (e) {
        if (open && !panel.contains(e.target) && !pill.contains(e.target)) {
            open = false;
            panel.classList.add('hidden');
            stopPolling();
        }
    });

    function startPolling() { stopPolling(); pollTimer = setInterval(pollActiveJobs, 3000); }
    function stopPolling() { if (pollTimer) { clearInterval(pollTimer); pollTimer = null; } }

    async function pollActiveJobs() {
        try {
            const r = await fetch('/api/jobs/active');
            if (!r.ok) return;
            const d = await r.json();
            const jobs = d.jobs || [];
            updatePill(jobs);
            updatePanel(jobs);
        } catch {}
    }

    function updatePill(jobs) {
        if (jobs.length > 0) {
            pill.classList.add('t-livepill--processing');
            pillText.textContent = 'Processing \u00b7 ' + jobs.length + ' job' + (jobs.length > 1 ? 's' : '');
        } else {
            pill.classList.remove('t-livepill--processing');
            if (typeof window.__thls_update_status_pill !== 'function') {
                pillText.textContent = 'Idle';
            }
        }
        pill.style.display = '';
    }

    function updatePanel(jobs) {
        if (!jobs.length) {
            list.innerHTML = '<div class="jobs-panel__empty">No active jobs</div>';
            return;
        }
        list.innerHTML = jobs.map(j => {
            const pct = Math.round(j.progress || 0);
            const statusLabel = j.status ? j.status.charAt(0).toUpperCase() + j.status.slice(1) : 'Unknown';
            const desc = j.description || statusLabel;
            return '<div class="jobs-panel__item">' +
                '<div class="jobs-panel__filename" title="' + escapeAttr(j.filename || j.job_id) + '">' + escapeHtml(j.filename || j.job_id) + '</div>' +
                '<div class="jobs-panel__status">' + escapeHtml(desc) + '</div>' +
                '<div class="jobs-panel__bar-bg"><div class="jobs-panel__bar" style="width:' + pct + '%"></div></div>' +
                '</div>';
        }).join('');
    }

    pollActiveJobs();
    const globalPollTimer = setInterval(pollActiveJobs, 5000);
    window.addEventListener('pagehide', () => clearInterval(globalPollTimer), { once: true });
    window.addEventListener('beforeunload', () => clearInterval(globalPollTimer), { once: true });
})();

// ─── Session auth and small user-data helpers ───────────────────────────────
(function () {
    const menu = document.getElementById('userMenu');
    const btn = document.getElementById('userMenuBtn');
    const popover = document.getElementById('userMenuPopover');
    const nameEl = document.getElementById('userMenuName');
    const logoutBtn = document.getElementById('logoutBtn');
    const loginForm = document.getElementById('loginForm');
    let currentUser = null;

    window.THLSAuth = {
        user: () => currentUser,
        isSignedIn: () => !!currentUser,
    };

    async function loadMe() {
        try {
            const resp = await fetch('/api/auth/me');
            if (!resp.ok) return null;
            const data = await resp.json();
            currentUser = data.authenticated ? data.user : null;
            updateUserMenu();
            return currentUser;
        } catch {
            return null;
        }
    }

    function updateUserMenu() {
        if (!menu || !nameEl || !logoutBtn) return;
        if (currentUser) {
            nameEl.textContent = currentUser.username || 'User';
            logoutBtn.hidden = false;
        } else {
            nameEl.textContent = 'Signed out';
            logoutBtn.hidden = true;
        }
    }

    if (btn && popover) {
        btn.addEventListener('click', (event) => {
            event.stopPropagation();
            popover.classList.toggle('hidden');
        });
        document.addEventListener('click', (event) => {
            if (!popover.contains(event.target) && event.target !== btn) {
                popover.classList.add('hidden');
            }
        });
    }

    if (logoutBtn) {
        logoutBtn.addEventListener('click', async () => {
            await fetch('/api/auth/logout', { method: 'POST' }).catch(() => {});
            window.location.href = '/login';
        });
    }

    if (loginForm) {
        loginForm.addEventListener('submit', async (event) => {
            event.preventDefault();
            const status = document.getElementById('loginStatus');
            const submit = document.getElementById('loginSubmit');
            if (status) status.textContent = '';
            if (submit) submit.disabled = true;
            try {
                const resp = await fetch('/api/auth/login', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        username: document.getElementById('loginUsername').value,
                        password: document.getElementById('loginPassword').value,
                    }),
                });
                if (!resp.ok) throw new Error('Invalid username or password');
                window.location.href = '/';
            } catch (error) {
                if (status) status.textContent = error.message || 'Sign in failed';
            } finally {
                if (submit) submit.disabled = false;
            }
        });
    }

    window.THLSUserData = {
        async toggleFavorite(jobId) {
            const resp = await fetch('/api/favorites/' + encodeURIComponent(jobId), { method: 'POST' });
            if (!resp.ok) throw new Error('Favorite update failed');
            return resp.json();
        },
        async toggleWatchlist(jobId) {
            const resp = await fetch('/api/watchlist/' + encodeURIComponent(jobId), { method: 'POST' });
            if (!resp.ok) throw new Error('Watchlist update failed');
            return resp.json();
        },
        async setRating(jobId, liked) {
            const resp = await fetch('/api/ratings/' + encodeURIComponent(jobId), {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: liked == null ? '' : JSON.stringify({ liked }),
            });
            if (!resp.ok) throw new Error('Rating update failed');
            return resp.json();
        },
    };

    loadMe();
})();
