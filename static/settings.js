// ─── Settings (dynamic, DB-backed) ────────────────────────────────────────────

let _settingsData = null;

function loadSettings() {
    fetch('/api/settings')
        .then(async r => {
            const data = await r.json().catch(() => ({}));
            if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
            return data;
        })
        .then(data => {
            _settingsData = data;
            renderAllSettings(data);
        })
        .catch((err) => {
            const host = document.getElementById('settings-server');
            if (host) host.innerHTML =
                `<div class="t-pane settings-pane"><div class="t-settings-row"><div style="color:var(--t-danger)">Failed to load settings: ${escHtml(err.message || 'Unknown error')}.</div></div></div>`;
        });
}

// ─── Sidebar / section navigation ────────────────────────────────────────────
//
// Each API settings category gets mapped to one of the static sections in
// the sidebar so the page matches the prototype's curated labels (General,
// Transcoding, ABR tiers, Cache, System, Cloudflared) rather than dumping
// raw category keys (file_handling, adaptive_bitrate, …).
const CATEGORY_TO_SECTION = {
    server:            'settings-server',
    file_handling:     'settings-cache',
    hardware:          'settings-system',
    hls:               'settings-media',
    adaptive_bitrate:  'settings-abr',
    reliability:       'settings-system',
    rate_limiting:     'settings-system',
    watch_folder:      'settings-watch',
    telegram:          'settings-bots',
    cloudflared:       'settings-cloudflared',
};

function renderAllSettings(data) {
    // Wipe any per-section dynamic groups the previous run left behind.
    document.querySelectorAll('.settings-group[data-dyn]').forEach(s => s.remove());
    const cats = data.categories || {};
    for (const [catKey, category] of Object.entries(cats)) {
        const targetId = CATEGORY_TO_SECTION[catKey] || 'settings-server';
        const target = document.getElementById(targetId);
        if (!target) continue;
        const groupEl = renderCategoryGroup(catKey, category);
        target.insertBefore(groupEl, target.firstChild);  // prepend dyn groups above any static markup
    }
    wireSidebar();
    activateSection('settings-server');
}

function wireSidebar() {
    document.querySelectorAll('#settingsSide .t-side__item').forEach(el => {
        el.onclick = () => activateSection(el.dataset.section);
    });
}
function activateSection(sectionId) {
    document.querySelectorAll('#settingsSide .t-side__item').forEach(el => {
        if (el.dataset.section === sectionId) el.setAttribute('aria-current', 'page');
        else el.removeAttribute('aria-current');
    });
    document.querySelectorAll('.settings-section').forEach(sec => {
        sec.hidden = sec.id !== sectionId;
    });
    const active = document.querySelector('#settingsSide .t-side__item[aria-current]');
    const heading = document.getElementById('settingsHeading');
    const sub = document.getElementById('settingsSubtitle');
    if (active && heading) heading.textContent = active.textContent.trim();
    if (sub) sub.textContent = sectionDescription(sectionId);
}
function sectionDescription(sectionId) {
    if (sectionId === 'settings-server')      return 'Network bindings, public URL, and global server flags.';
    if (sectionId === 'settings-bots')        return 'Add, remove, and probe Telegram storage bots.';
    if (sectionId === 'settings-watch')       return 'Auto-ingest media dropped into a watched folder.';
    if (sectionId === 'settings-db')          return 'Backup, export, import, and replace the SQLite database.';
    if (sectionId === 'settings-media')       return 'Transcoding pipeline, HLS, and encoder settings.';
    if (sectionId === 'settings-abr')         return 'Adaptive bitrate tier behaviour.';
    if (sectionId === 'settings-cache')       return 'Segment cache and disk-backed cache.';
    if (sectionId === 'settings-system')      return 'Reliability, rate limiting, and hardware acceleration.';
    if (sectionId === 'settings-cloudflared') return 'Cloudflared tunnel configuration.';
    return 'Configure server behaviour. Changes save per section.';
}

// ─── Category group (dynamic from /api/settings) ─────────────────────────────
// Returns a `.settings-group` to be inserted into one of the static sections.
function renderCategoryGroup(catKey, category) {
    const group = document.createElement('div');
    group.className = 'settings-group';
    group.dataset.category = catKey;
    group.dataset.dyn = '1';

    const head = document.createElement('div');
    head.className = 'settings-group-head';
    head.innerHTML = `<h2>${escHtml(category.label)}</h2>`;
    group.appendChild(head);

    const pane = document.createElement('div');
    pane.className = 't-pane settings-pane';
    for (const setting of category.settings) pane.appendChild(renderFieldRow(setting));
    group.appendChild(pane);

    const actions = document.createElement('div');
    actions.className = 'settings-actions';
    const saveBtn = document.createElement('button');
    saveBtn.className = 'action-btn primary';
    saveBtn.textContent = 'Save';
    const statusEl = document.createElement('span');
    statusEl.className = 'settings-status';
    saveBtn.onclick = () => saveCategory(catKey, saveBtn, statusEl);
    actions.append(saveBtn, statusEl);
    group.appendChild(actions);

    return group;
}

function renderFieldRow(setting) {
    const row = document.createElement('div');
    row.className = 't-settings-row';
    if (setting.type === 'tiers') row.classList.add('t-settings-row--stack');
    row.dataset.key = setting.key;

    const left = document.createElement('div');
    const titleEl = document.createElement('div');
    titleEl.className = 't-settings-row-label';
    titleEl.textContent = setting.key;
    const hintEl = document.createElement('div');
    hintEl.className = 't-settings-row-hint';
    const desc = setting.description || '';
    const formattedDefault = formatDisplay(setting.key, setting.default);
    hintEl.textContent = desc + (desc ? ' · ' : '') +
        'Default: ' + setting.default + (formattedDefault ? ' ' + formattedDefault : '');
    left.append(titleEl, hintEl);
    row.appendChild(left);

    const right = document.createElement('div');
    right.style.cssText = 'display:flex;align-items:center;gap:8px;justify-content:flex-end;';
    if (setting.type === 'bool') {
        const sw = document.createElement('button');
        sw.type = 'button';
        sw.className = 't-switch';
        sw.id = `sf_${setting.key}`;
        sw.setAttribute('role', 'switch');
        sw.setAttribute('aria-checked', String(!!setting.value));
        sw.addEventListener('click', () => {
            const on = sw.getAttribute('aria-checked') !== 'true';
            sw.setAttribute('aria-checked', String(on));
        });
        right.appendChild(sw);
    } else if (setting.type === 'tiers') {
        const ta = document.createElement('textarea');
        ta.id = `sf_${setting.key}`;
        ta.className = 't-input';
        ta.rows = 2;
        ta.value = setting.value || '';
        ta.style.minWidth = '0';
        right.style.justifyContent = 'stretch';
        right.appendChild(ta);
    } else {
        const inp = document.createElement('input');
        inp.id = `sf_${setting.key}`;
        inp.className = 't-input';
        inp.type = setting.type === 'int' ? 'number' : 'text';
        inp.value = setting.value !== null && setting.value !== undefined ? String(setting.value) : '';
        if (setting.type === 'int') inp.style.width = '160px';
        right.appendChild(inp);
    }
    const reset = document.createElement('button');
    reset.type = 'button';
    reset.className = 'action-btn';
    reset.title = `Reset to default: ${setting.default}`;
    reset.textContent = 'Reset';
    reset.style.fontSize = '12px';
    reset.style.padding = '4px 10px';
    reset.onclick = () => resetSetting(setting.key);
    right.appendChild(reset);

    row.appendChild(right);
    return row;
}

function collectCategoryValues(catKey) {
    const result = {};
    const cat = _settingsData?.categories?.[catKey];
    if (!cat) return {};
    for (const setting of cat.settings) {
        const el = document.getElementById(`sf_${setting.key}`);
        if (!el) continue;
        if (setting.type === 'bool') {
            result[setting.key] = el.getAttribute('aria-checked') === 'true';
        } else if (setting.type === 'int') {
            result[setting.key] = parseInt(el.value, 10);
        } else {
            result[setting.key] = el.value;
        }
    }
    return result;
}

function saveCategory(catKey, btn, statusEl) {
    const values = collectCategoryValues(catKey);
    btn.disabled = true;
    statusEl.textContent = 'Saving…';
    statusEl.className = 'settings-status';
    fetch('/api/settings', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(values),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Save failed');
        _settingsData = d;
        statusEl.textContent = 'Saved.';
        statusEl.className = 'settings-status ok';
    }).catch(e => {
        statusEl.textContent = e.message;
        statusEl.className = 'settings-status error';
    }).finally(() => {
        btn.disabled = false;
    });
}

function resetSetting(key) {
    if (!confirm(`Reset "${key}" to its default value?`)) return;
    fetch('/api/settings/reset', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ keys: [key] }),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Reset failed');
        _settingsData = d;
        renderAllSettings(d);
    }).catch(e => alert(`Reset failed: ${e.message}`));
}

// ─── Bot Management ────────────────────────────────────────────────────────────

function loadBots() {
    fetch('/api/bots')
        .then(r => r.json())
        .then(data => renderBotList(data.bots))
        .catch(() => {
            document.getElementById('botListContainer').innerHTML =
                '<div class="bot-empty" style="color:var(--danger)">Failed to load bots.</div>';
        });
}

function renderBotList(bots) {
    const container = document.getElementById('botListContainer');
    container.classList.add('bot-list-pane');
    if (!bots || bots.length === 0) {
        container.innerHTML = '<div class="bot-empty" style="color:var(--t-ink-3);font-size:13px;padding:18px;">No bots configured. Add one below.</div>';
        return;
    }
    container.innerHTML = '';
    for (const bot of bots) container.appendChild(renderBotRow(bot));
}

function renderBotRow(bot) {
    const row = document.createElement('div');
    row.className = 'bot-row';
    row.id = `bot-row-${bot.index}`;
    const stats = bot.stats || {};
    const sessionLabel = stats.session_uploads > 0
        ? `${stats.session_uploads} uploads · ${formatBytes(stats.session_upload_bytes || 0)}`
        : 'No uploads this session';
    const segments = (stats.segment_count || 0).toLocaleString();
    const storage = formatBytes(stats.total_bytes || 0);
    const meta = [
        `<span>${escHtml(bot.channel_id)}</span>`,
        `<span class="sep"></span><span>${segments} segments</span>`,
        `<span class="sep"></span><span>${storage}</span>`,
        `<span class="sep"></span><span title="${escHtml(sessionLabel)}">${stats.session_uploads > 0 ? sessionLabel : 'idle'}</span>`,
    ].join('');
    row.innerHTML = `
        <div class="bot-index">${bot.index}</div>
        <div style="min-width:0">
            <div class="bot-token">${escHtml(bot.token_masked)}${bot.source === 'env' ? ' <span style="font-size:11px;color:var(--t-ink-3)">(env)</span>' : ''}${bot.label ? ` <span style="color:var(--t-ink-3)">— ${escHtml(bot.label)}</span>` : ''}</div>
            <div class="bot-meta">${meta}</div>
        </div>
        <div style="display:flex;align-items:center;gap:6px;font-size:12px;color:var(--t-ink-2)">
            <span class="bot-status-dot" id="bot-dot-${bot.index}"></span>
            <span id="bot-status-${bot.index}">—</span>
        </div>
        <div class="bot-actions">
            <button class="action-btn" onclick="checkBotHealth(${bot.index})">Check</button>
            ${bot.source === 'db' && bot.db_id != null
                ? `<button class="action-btn danger" onclick="deleteBot(${bot.db_id})">Delete</button>`
                : `<button class="action-btn" disabled title="Remove from .env to delete">Delete</button>`
            }
        </div>
        <div></div>
    `;
    return row;
}

function updateBotStatusUI(index, result) {
    const dot = document.getElementById(`bot-dot-${index}`);
    const label = document.getElementById(`bot-status-${index}`);
    if (!dot || !label) return;
    dot.className = 'bot-status-dot ' + (result.ok ? 'ok' : 'error');
    label.textContent = result.ok ? 'OK' : (result.error || 'Error');
    label.style.color = result.ok ? 'var(--success)' : 'var(--danger)';
}

function checkBotHealth(index) {
    const dot = document.getElementById(`bot-dot-${index}`);
    const label = document.getElementById(`bot-status-${index}`);
    if (dot) dot.className = 'bot-status-dot checking';
    if (label) { label.textContent = 'Checking…'; label.style.color = 'var(--text-muted)'; }

    fetch('/api/bots/health', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ index }),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Health check failed');
        const result = d.results?.[0];
        if (result) updateBotStatusUI(index, result);
    }).catch(e => {
        if (dot) dot.className = 'bot-status-dot error';
        if (label) { label.textContent = e.message; label.style.color = 'var(--danger)'; }
    });
}

function checkAllBotHealth() {
    const statusEl = document.getElementById('botHealthStatus');
    statusEl.textContent = 'Checking all…';
    statusEl.className = 'settings-status';

    // Set all dots to checking state
    document.querySelectorAll('.bot-status-dot').forEach(d => d.className = 'bot-status-dot checking');

    fetch('/api/bots/health', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({}),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Health check failed');
        for (const result of (d.results || [])) {
            updateBotStatusUI(result.index, result);
        }
        const ok = d.results?.every(r => r.ok);
        statusEl.textContent = ok ? 'All bots OK' : 'Some bots have errors';
        statusEl.className = 'settings-status ' + (ok ? 'ok' : 'error');
    }).catch(e => {
        statusEl.textContent = e.message;
        statusEl.className = 'settings-status error';
        document.querySelectorAll('.bot-status-dot').forEach(d => d.className = 'bot-status-dot error');
    });
}

function deleteBot(dbId) {
    if (!confirm('Remove this bot? It will no longer be used for uploads.')) return;
    fetch(`/api/bots/${dbId}`, { method: 'DELETE' })
        .then(async r => {
            const d = await r.json();
            if (!r.ok) throw new Error(d.error || 'Delete failed');
            loadBots();
        })
        .catch(e => alert(`Delete failed: ${e.message}`));
}

// ─── Add Bot Modal ─────────────────────────────────────────────────────────────

function openAddBotModal() {
    document.getElementById('newBotToken').value = '';
    document.getElementById('newBotChannelId').value = '';
    document.getElementById('newBotLabel').value = '';
    document.getElementById('addBotStatus').textContent = '';
    document.getElementById('addBotStatus').className = 'settings-status';
    document.getElementById('addBotModal').classList.add('active');
}

function closeAddBotModal() {
    document.getElementById('addBotModal').classList.remove('active');
}

function handleModalOverlayClick(e) {
    if (e.target === document.getElementById('addBotModal')) closeAddBotModal();
}

function testAndSaveBot() {
    const token = document.getElementById('newBotToken').value.trim();
    const channelId = document.getElementById('newBotChannelId').value.trim();
    const label = document.getElementById('newBotLabel').value.trim();
    const statusEl = document.getElementById('addBotStatus');
    const btn = document.getElementById('addBotSaveBtn');

    if (!token || !channelId) {
        statusEl.textContent = 'Token and Channel ID are required.';
        statusEl.className = 'settings-status error';
        return;
    }

    btn.disabled = true;
    statusEl.textContent = 'Testing…';
    statusEl.className = 'settings-status';

    fetch('/api/bots/add', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token, channel_id: parseInt(channelId, 10), label }),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Failed to add bot');
        statusEl.textContent = 'Bot added successfully!';
        statusEl.className = 'settings-status ok';
        setTimeout(() => {
            closeAddBotModal();
            loadBots();
        }, 800);
    }).catch(e => {
        statusEl.textContent = e.message;
        statusEl.className = 'settings-status error';
    }).finally(() => {
        btn.disabled = false;
    });
}

// ─── Watch Settings ────────────────────────────────────────────────────────────

function setWatchSettingsStatus(msg, kind='') {
    const el = document.getElementById('watchSettingsStatus');
    el.textContent = msg;
    el.className = `settings-status${kind ? ' '+kind : ''}`;
}

function applyWatchSettings(data) {
    document.getElementById('watchEnabled').checked = !!data.watch_enabled;
    const sw = document.getElementById('watchEnabledSwitch');
    if (sw) sw.setAttribute('aria-checked', String(!!data.watch_enabled));
    document.getElementById('watchRoot').value = data.watch_root || '';
    document.getElementById('watchDoneDir').value = data.watch_done_dir || '';
    if (data.watch_enabled) {
        setWatchSettingsStatus(`Saved. ${data.watch_running ? 'Watcher active.' : 'Watcher pending.'}`, 'ok');
    } else { setWatchSettingsStatus('Watcher disabled.'); }
}

function setupWatchSwitch() {
    const sw = document.getElementById('watchEnabledSwitch');
    const cb = document.getElementById('watchEnabled');
    if (!sw || !cb) return;
    sw.addEventListener('click', () => {
        const on = sw.getAttribute('aria-checked') !== 'true';
        sw.setAttribute('aria-checked', String(on));
        cb.checked = on;
    });
}

function loadWatchSettings() {
    fetch('/api/watch-settings').then(r=>r.json()).then(applyWatchSettings)
        .catch(()=>setWatchSettingsStatus('Could not load settings.','error'));
}

function saveWatchSettings() {
    const btn = document.getElementById('saveWatchSettingsBtn');
    btn.disabled = true;
    setWatchSettingsStatus('Saving…','');
    fetch('/api/watch-settings', {
        method:'POST', headers:{'Content-Type':'application/json'},
        body: JSON.stringify({
            watch_enabled: document.getElementById('watchEnabled').checked,
            watch_root: document.getElementById('watchRoot').value.trim(),
            watch_done_dir: document.getElementById('watchDoneDir').value.trim(),
        }),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Could not save.');
        applyWatchSettings(d);
    }).catch(e=>setWatchSettingsStatus(e.message,'error')).finally(()=>{ btn.disabled=false; });
}

// ─── Database Load ─────────────────────────────────────────────────────────────

function setDbExportStatus(msg, kind='') {
    const el = document.getElementById('dbExportStatus');
    if (!el) return;
    el.textContent = msg;
    el.className = `settings-status${kind ? ' '+kind : ''}`;
}

function setDbImportStatus(msg, kind='') {
    const el = document.getElementById('dbImportStatus');
    if (!el) return;
    el.textContent = msg;
    el.className = `settings-status${kind ? ' '+kind : ''}`;
}

function downloadDbExport() {
    setDbExportStatus('Preparing…');
    fetch('/api/db/export', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ upload_to_telegram: false }),
    }).then(async r => {
        if (!r.ok) {
            const d = await r.json().catch(() => ({}));
            throw new Error(d.message || d.error || 'Export failed');
        }
        const blob = await r.blob();
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'streamer-export.json';
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
        setDbExportStatus('Export downloaded.', 'ok');
    }).catch(e => setDbExportStatus(e.message, 'error'));
}

function backupDatabase() {
    setDbExportStatus('Creating backup…');
    fetch('/api/db/backup', { method: 'POST' })
        .then(async r => {
            const d = await r.json().catch(() => ({}));
            if (!r.ok) throw new Error(d.message || d.error || 'Backup failed');
            setDbExportStatus(`Backup saved at ${d.backup_path}`, 'ok');
        })
        .catch(e => setDbExportStatus(e.message, 'error'));
}

function telegramDbExport() {
    setDbExportStatus('Uploading…');
    fetch('/api/db/export', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ upload_to_telegram: true }),
    }).then(async r => {
        const d = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(d.message || d.error || 'Export failed');
        const fileIdInput = document.getElementById('telegramImportFileId');
        const botIndexInput = document.getElementById('telegramImportBotIndex');
        if (fileIdInput && typeof d.file_id === 'string') fileIdInput.value = d.file_id;
        if (botIndexInput && Number.isFinite(Number(d.bot_index))) {
            botIndexInput.value = String(d.bot_index);
        }
        setDbExportStatus(`Uploaded with bot ${d.bot_index}: ${d.file_id}`, 'ok');
    }).catch(e => setDbExportStatus(e.message, 'error'));
}

function readBotIndexMap() {
    const raw = document.getElementById('dbImportMap').value.trim();
    if (!raw) return {};
    let parsed;
    try {
        parsed = JSON.parse(raw);
    } catch {
        throw new Error('Bot index map must be valid JSON object, e.g. {"0":0,"1":0}. Optional: leave empty for auto-mapping.');
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        throw new Error('Bot index map must be a JSON object like {"0":0,"1":0}.');
    }
    for (const [key, value] of Object.entries(parsed)) {
        if (!/^[-]?\d+$/.test(key) || !Number.isInteger(value)) {
            throw new Error(`Invalid bot_index_map entry: "${key}": ${JSON.stringify(value)}. Keys and values must be integers.`);
        }
    }
    return parsed;
}

function importDbExportFile() {
    const file = document.getElementById('dbImportFileInput')?.files?.[0];
    if (!file) {
        setDbImportStatus('Choose a local export JSON file first.', 'error');
        return;
    }
    const map = readBotIndexMap();
    const formData = new FormData();
    formData.append('file', file);
    formData.append('bot_index_map', JSON.stringify(map));
    setDbImportStatus('Importing…');
    fetch('/api/db/import', { method: 'POST', body: formData })
        .then(async r => {
            const d = await r.json().catch(() => ({}));
            if (!r.ok) throw new Error(d.message || d.error || 'Import failed');
            setDbImportStatus(`Imported ${d.merged_jobs} jobs and ${d.merged_segments} segments.`, 'ok');
        })
        .catch(e => setDbImportStatus(e.message, 'error'));
}

function importDbExportTelegram() {
    const map = readBotIndexMap();
    const fileId = document.getElementById('telegramImportFileId').value.trim();
    const botIndex = parseInt(document.getElementById('telegramImportBotIndex').value, 10);
    if (!fileId) {
        setDbImportStatus('Telegram file_id is required.', 'error');
        return;
    }
    if (Number.isNaN(botIndex)) {
        setDbImportStatus('Telegram downloader bot index must be a number.', 'error');
        return;
    }
    setDbImportStatus('Downloading…');
    fetch('/api/db/import', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ file_id: fileId, bot_index: botIndex, bot_index_map: map }),
    }).then(async r => {
        const d = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(d.message || d.error || 'Import failed');
        setDbImportStatus(`Imported ${d.merged_jobs} jobs and ${d.merged_segments} segments.`, 'ok');
    }).catch(e => setDbImportStatus(e.message, 'error'));
}

function loadDatabaseFromFile() {
    const fileInput = document.getElementById('databaseFileInput');
    const statusEl = document.getElementById('databaseLoadStatus');
    const btn = document.getElementById('databaseLoadBtn');
    const file = fileInput?.files?.[0];

    if (!file) {
        statusEl.textContent = 'Choose a database file first.';
        statusEl.className = 'settings-status error';
        return;
    }

    const formData = new FormData();
    formData.append('database', file);
    btn.disabled = true;
    statusEl.textContent = 'Loading database…';
    statusEl.className = 'settings-status';

    fetch('/api/database/load', {
        method: 'POST',
        body: formData,
    }).then(async (r) => {
        const data = await r.json().catch(() => ({}));
        if (!r.ok) throw new Error(data.error || 'Failed to load database');
        statusEl.textContent = `Loaded. Backup saved at ${data.backup_path || 'server backup path unavailable'}.`;
        statusEl.className = 'settings-status ok';
        loadSettings();
        loadBots();
    }).catch((err) => {
        statusEl.textContent = err.message || 'Failed to load database';
        statusEl.className = 'settings-status error';
    }).finally(() => {
        btn.disabled = false;
    });
}

// ─── Utilities ─────────────────────────────────────────────────────────────────

function formatDisplay(key, value) {
    const num = Number(value);
    if (isNaN(num)) return '';
    // Bytes
    if (/_SIZE$|_BYTES$/.test(key)) {
        if (num >= 1073741824) return '(' + (num / 1073741824).toFixed(1) + ' GB)';
        if (num >= 1048576) return '(' + (num / 1048576).toFixed(1) + ' MB)';
        if (num >= 1024) return '(' + (num / 1024).toFixed(1) + ' KB)';
        return '(' + num + ' bytes)';
    }
    // Seconds
    if (/_SECONDS$/.test(key)) {
        if (num >= 86400) return '(' + (num / 86400).toFixed(1) + ' days)';
        if (num >= 3600) return '(' + (num / 3600).toFixed(1) + ' hours)';
        if (num >= 60) return '(' + (num / 60).toFixed(0) + ' min)';
        return '(' + num + ' sec)';
    }
    // Days (0 = forever)
    if (/_DAYS$/.test(key)) {
        if (num === 0) return '(forever)';
        return '(' + num + ' day' + (num === 1 ? '' : 's') + ')';
    }
    // Minutes
    if (/_MINUTES$/.test(key)) {
        if (num === 0) return '(disabled)';
        return '(' + num + ' min)';
    }
    // MB already labelled in key name
    if (/_MB$/.test(key)) return '(' + num.toLocaleString() + ' megabytes)';
    return '';
}

function escHtml(str) {
    return String(str).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

// ─── Init ──────────────────────────────────────────────────────────────────────

loadSettings();
loadBots();
loadWatchSettings();
setupWatchSwitch();
