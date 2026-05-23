// ─── Settings (dynamic, DB-backed) ────────────────────────────────────────────

let _settingsData = null;
let _tierModalState = null;

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
    const activeSection = document.querySelector('#settingsSide .t-side__item[aria-current]')?.dataset.section || 'settings-server';
    document.querySelectorAll('.settings-group[data-dyn]').forEach(s => s.remove());
    const cats = data.categories || {};
    const sectionMap = {};
    for (const [catKey, category] of Object.entries(cats)) {
        const targetId = CATEGORY_TO_SECTION[catKey] || 'settings-server';
        if (!sectionMap[targetId]) sectionMap[targetId] = [];
        sectionMap[targetId].push({ catKey, category });
    }
    for (const [sectionId, catEntries] of Object.entries(sectionMap)) {
        const target = document.getElementById(sectionId);
        if (!target) continue;
        const groupEl = renderSectionGroup(sectionId, catEntries);
        target.insertBefore(groupEl, target.firstChild);
    }
    wireSidebar();
    activateSection(activeSection);
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
    if (active && heading) heading.innerHTML = active.innerHTML;
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

function renderSectionGroup(sectionId, catEntries) {
    const group = document.createElement('div');
    group.className = 'settings-group';
    group.dataset.dyn = '1';
    const catKeys = catEntries.map(e => e.catKey);

    for (const { catKey, category } of catEntries) {
        const pane = document.createElement('div');
        pane.className = 't-pane settings-pane';

        const subhead = document.createElement('div');
        subhead.className = 'settings-subhead';

        const labelText = document.createElement('span');
        labelText.textContent = category.label;
        subhead.appendChild(labelText);

        const subheadActions = document.createElement('div');
        subheadActions.className = 'subhead-actions';

        const statusEl = document.createElement('span');
        statusEl.className = 'settings-status subhead-status';

        const saveBtn = document.createElement('button');
        saveBtn.className = 'subhead-save-btn';
        saveBtn.textContent = 'Save';
        saveBtn.onclick = () => saveSection([catKey], saveBtn, statusEl);

        subheadActions.append(statusEl, saveBtn);
        subhead.appendChild(subheadActions);
        pane.appendChild(subhead);

        for (const setting of category.settings) {
            pane.appendChild(renderFieldRow(setting));
        }
        group.appendChild(pane);
    }

    return group;
}

const SETTING_LABELS = {
    HOST: 'Bind address',
    PORT: 'Port',
    FORCE_HTTPS: 'Force HTTPS',
    BEHIND_PROXY: 'Behind proxy',
    TRUSTED_PROXY_CIDRS: 'Trusted proxy ranges',
    CORS_ALLOWED_ORIGINS: 'Allowed CORS origins',
    TELEGRAM_MAX_FILE_SIZE: 'Telegram file limit',
    MAX_UPLOAD_SIZE: 'Maximum upload size',
    UPLOAD_CHUNK_SIZE: 'Upload chunk size',
    SEGMENT_TARGET_SIZE: 'Target segment size',
    SEGMENT_CACHE_SIZE_MB: 'Segment cache budget',
    SEGMENT_PREFETCH_COUNT: 'Prefetch count',
    SEGMENT_PREFETCH_MIN_FREE_BYTES: 'Minimum free cache space',
    ENABLE_HW_ACCEL: 'Hardware acceleration',
    PREFERRED_ENCODER: 'Preferred encoder',
    VAAPI_DEVICE: 'VAAPI device',
    MAX_PARALLEL_ENCODES: 'Parallel encodes',
    VIDEO_BITRATE: 'Default video bitrate',
    AUDIO_BITRATE: 'Default audio bitrate',
    AUDIO_SEGMENT_DURATION: 'Audio segment duration',
    ABR_ENABLED: 'Create ABR variants',
    ENABLE_COPY_MODE: 'Keep source tier',
    VIRTUAL_ABR_TIERS: 'On-demand ABR',
    ABR_TIERS: 'Playback ABR tiers',
    TIER0_BITRATES: 'Source tier bitrates',
    TIER0_BITRATE_DEFAULT: 'Default source tier bitrate',
    JOB_TIMEOUT_SECONDS: 'Job timeout',
    QUEUE_TIMEOUT_SECONDS: 'Queue timeout',
    PENDING_UPLOAD_TTL_SECONDS: 'Pending upload expiry',
    PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS: 'Pending cleanup interval',
    JOB_RETENTION_DAYS: 'Job retention',
    MAX_CONCURRENT_JOBS: 'Concurrent jobs',
    UPLOAD_RATE_LIMIT_WINDOW: 'Rate limit window',
    UPLOAD_RATE_LIMIT_MAX_REQUESTS: 'Rate limit requests',
    MAX_PENDING_UPLOADS_PER_IP: 'Pending uploads per IP',
    WATCH_POLL_SECONDS: 'Watch scan interval',
    WATCH_STABLE_SECONDS: 'File stability delay',
    WATCH_VIDEO_EXTENSIONS: 'Video extensions',
    WATCH_IGNORE_SUFFIXES: 'Ignored suffixes',
    UPLOAD_PARALLELISM: 'Upload parallelism',
    DB_SYNC_ENABLED: 'Sync database to bots',
    DB_SYNC_BOOTSTRAP: 'DB sync bootstrap',
    DB_AUTO_MERGE_INTERVAL_MINUTES: 'Auto-merge interval',
    DB_AUTO_MERGE_FILE_ID: 'Auto-merge file ID',
    DB_AUTO_MERGE_BOT_INDEX: 'Auto-merge bot index',
    WEBHOOK_URL: 'Webhook URL',
    CLOUDFLARED_ENABLED: 'Cloudflared enabled',
    CLOUDFLARED_CONFIG: 'Cloudflared config path',
};

function humanizeKey(key) {
    if (SETTING_LABELS[key]) return SETTING_LABELS[key];
    return key.toLowerCase().split('_').map(part => part.charAt(0).toUpperCase() + part.slice(1)).join(' ');
}

function unitKind(key) {
    if (/_SIZE$|_BYTES$/.test(key)) return 'bytes';
    if (/_SECONDS$/.test(key)) return 'seconds';
    if (/_DAYS$/.test(key)) return 'days';
    if (/_MINUTES$/.test(key)) return 'minutes';
    if (/_MB$/.test(key)) return 'mb';
    return null;
}

function humanizeUnit(kind, num) {
    if (isNaN(num)) return '';
    switch (kind) {
        case 'bytes':
            if (num >= 1073741824) return (num / 1073741824).toFixed(1) + ' GB';
            if (num >= 1048576) return (num / 1048576).toFixed(1) + ' MB';
            if (num >= 1024) return (num / 1024).toFixed(1) + ' KB';
            return num + ' B';
        case 'seconds':
            if (num >= 86400 && num % 86400 === 0) return (num / 86400) + ' days';
            if (num >= 86400) return (num / 86400).toFixed(1) + ' days';
            if (num >= 3600 && num % 3600 === 0) return (num / 3600) + 'h';
            if (num >= 3600) return (num / 3600).toFixed(1) + 'h';
            if (num >= 60 && num % 60 === 0) return (num / 60) + ' min';
            if (num >= 60) return (num / 60).toFixed(0) + ' min';
            return num + 's';
        case 'days':
            if (num === 0) return 'forever';
            return num + (num === 1 ? ' day' : ' days');
        case 'minutes':
            if (num === 0) return 'disabled';
            return num + ' min';
        case 'mb':
            return num.toLocaleString() + ' MB';
    }
    return '';
}

function unitChoices(kind) {
    if (kind === 'bytes') return [
        ['1', 'B'], ['1024', 'KB'], ['1048576', 'MB'], ['1073741824', 'GB'],
    ];
    if (kind === 'seconds') return [
        ['1', 'sec'], ['60', 'min'], ['3600', 'hr'], ['86400', 'day'],
    ];
    if (kind === 'minutes') return [
        ['1', 'min'], ['60', 'hr'], ['1440', 'day'],
    ];
    if (kind === 'days') return [['1', 'day']];
    if (kind === 'mb') return [['1', 'MB']];
    return [['1', '']];
}

function bestUnit(kind, rawValue) {
    const choices = unitChoices(kind);
    const value = Math.abs(Number(rawValue) || 0);
    let best = choices[0];
    for (const choice of choices) {
        const factor = Number(choice[0]);
        if (value >= factor && value % factor === 0) best = choice;
    }
    return best;
}

function renderFriendlyInt(setting, container, kind) {
    const hidden = document.createElement('input');
    hidden.type = 'hidden';
    hidden.id = 'sf_' + setting.key;
    hidden.dataset.settingKind = 'friendly-int';
    hidden.dataset.unitKind = kind;

    const current = Number(setting.value);
    const [factor, label] = bestUnit(kind, current);
    hidden.value = Number.isFinite(current) ? String(current) : '';

    const wrap = document.createElement('div');
    wrap.className = 'settings-unit-input';

    const number = document.createElement('input');
    number.className = 't-input settings-unit-number';
    number.type = 'number';
    number.min = '0';
    number.step = '1';
    number.value = Number.isFinite(current) ? String(current / Number(factor)) : '';

    const select = document.createElement('select');
    select.className = 't-input settings-unit-select';
    for (const [choiceFactor, choiceLabel] of unitChoices(kind)) {
        const opt = document.createElement('option');
        opt.value = choiceFactor;
        opt.textContent = choiceLabel;
        if (choiceFactor === factor && choiceLabel === label) opt.selected = true;
        select.appendChild(opt);
    }
    const sync = () => {
        const n = Number(number.value);
        const f = Number(select.value);
        if (!Number.isFinite(n)) {
            hidden.value = '';
            return;
        }
        const raw = Math.round(n * f);
        hidden.value = String(raw);
    };
    number.addEventListener('input', sync);
    select.addEventListener('change', sync);
    sync();

    wrap.append(number, select);
    container.append(hidden, wrap);
}

function renderFieldRow(setting) {
    const row = document.createElement('div');
    row.className = 't-settings-row';
    if (setting.type === 'tiers') row.classList.add('t-settings-row--stack');
    row.dataset.key = setting.key;

    const kind = unitKind(setting.key);
    const left = document.createElement('div');
    const titleEl = document.createElement('div');
    titleEl.className = 't-settings-row-label';
    titleEl.textContent = humanizeKey(setting.key);
    titleEl.title = setting.key;
    const hintEl = document.createElement('div');
    hintEl.className = 't-settings-row-hint';
    const desc = setting.description || '';
    if (kind) {
        const humanDefault = humanizeUnit(kind, Number(setting.default));
        hintEl.textContent = desc + (desc ? ' · ' : '') + 'Default: ' + humanDefault + ` (${setting.key})`;
    } else {
        const formattedDefault = formatDisplay(setting.key, setting.default);
        hintEl.textContent = desc + (desc ? ' · ' : '') +
            'Default: ' + setting.default + (formattedDefault ? ' ' + formattedDefault : '') + ` (${setting.key})`;
    }
    left.append(titleEl, hintEl);
    row.appendChild(left);

    const right = document.createElement('div');
    right.className = 'settings-field-control';

    const reset = document.createElement('button');
    reset.type = 'button';
    reset.className = 'action-btn';
    reset.title = 'Reset to default: ' + setting.default;
    reset.textContent = 'Reset';
    reset.style.fontSize = '12px';
    reset.style.padding = '4px 10px';
    reset.onclick = () => resetSetting(setting.key);

    if (setting.type === 'bool') {
        const sw = document.createElement('button');
        sw.type = 'button';
        sw.className = 't-switch';
        sw.id = 'sf_' + setting.key;
        sw.setAttribute('role', 'switch');
        sw.setAttribute('aria-checked', String(!!setting.value));
        sw.addEventListener('click', () => {
            const on = sw.getAttribute('aria-checked') !== 'true';
            sw.setAttribute('aria-checked', String(on));
        });
        right.appendChild(sw);
        right.appendChild(reset);
    } else if (setting.type === 'tiers') {
        right.classList.add('settings-field-control--stack');
        buildTiersEditor(setting, right);
    } else {
        if (kind && setting.type === 'int') {
            renderFriendlyInt(setting, right, kind);
        } else {
            const inp = document.createElement('input');
            inp.id = 'sf_' + setting.key;
            inp.className = 't-input';
            inp.type = setting.type === 'int' ? 'number' : 'text';
            inp.value = Array.isArray(setting.value)
                ? setting.value.join(',')
                : (setting.value !== null && setting.value !== undefined ? String(setting.value) : '');
            if (setting.type === 'int') inp.classList.add('settings-number-input');
            right.appendChild(inp);
        }
        right.appendChild(reset);
    }

    row.appendChild(right);
    return row;
}

function parseTiers(val) {
    if (!val) return [];
    return val.split(',').map(pair => {
        const parts = pair.trim().split(':');
        return { height: parts[0] ? parseInt(parts[0], 10) : 0, bitrate: (parts[1] || '').trim() };
    }).filter(t => t.height > 0 || t.bitrate);
}

function serializeTiers(tiers) {
    return tiers
        .filter(t => Number(t.height) > 0 && String(t.bitrate || '').trim())
        .map(t => `${parseInt(t.height, 10)}:${String(t.bitrate).trim()}`)
        .join(',');
}

function buildTiersEditor(setting, container) {
    const hiddenInput = document.createElement('input');
    hiddenInput.type = 'hidden';
    hiddenInput.id = 'sf_' + setting.key;
    hiddenInput.value = setting.value || '';
    hiddenInput.dataset.settingKind = 'tiers';

    const wrapper = document.createElement('div');
    wrapper.className = 'tier-editor';
    wrapper.dataset.key = setting.key;
    wrapper.dataset.tiers = JSON.stringify(parseTiers(setting.value));

    renderTierList(wrapper, hiddenInput);

    // Create a row for ABR tier editor bottom actions (Add tier + Reset)
    const btnRow = document.createElement('div');
    btnRow.style.cssText = 'display: flex; align-items: center; gap: 8px; margin-top: 10px;';

    const addBtn = document.createElement('button');
    addBtn.type = 'button';
    addBtn.className = 'action-btn';
    addBtn.innerHTML = '<span class="material-icons-round" style="font-size:16px">add</span> Add tier';
    addBtn.onclick = () => openTierModal(wrapper, hiddenInput, null);

    const resetBtn = document.createElement('button');
    resetBtn.type = 'button';
    resetBtn.className = 'action-btn';
    resetBtn.title = 'Reset to default: ' + setting.default;
    resetBtn.innerHTML = '<span class="material-icons-round" style="font-size:16px">restart_alt</span> Reset';
    resetBtn.onclick = () => resetSetting(setting.key);

    btnRow.append(addBtn, resetBtn);
    wrapper.appendChild(btnRow);

    container.appendChild(hiddenInput);
    container.appendChild(wrapper);
}

function tierRole(key) {
    return key === 'TIER0_BITRATES'
        ? 'Source tier passthrough'
        : 'Encoded playback tier';
}

function getTierData(wrapper) {
    try { return JSON.parse(wrapper.dataset.tiers || '[]'); }
    catch (_) { return []; }
}

function setTierData(wrapper, hiddenInput, tiers) {
    wrapper.dataset.tiers = JSON.stringify(tiers);
    hiddenInput.value = serializeTiers(tiers);
    renderTierList(wrapper, hiddenInput);
}

function renderTierList(wrapper, hiddenInput) {
    wrapper.querySelectorAll('.tier-editor-list, .tier-empty').forEach(el => el.remove());
    const addBtn = wrapper.querySelector('.tier-editor-add');
    const tiers = getTierData(wrapper);
    if (!tiers.length) {
        const empty = document.createElement('div');
        empty.className = 'tier-empty';
        empty.textContent = 'No tiers configured.';
        wrapper.insertBefore(empty, addBtn || null);
        return;
    }
    const list = document.createElement('div');
    list.className = 'tier-editor-list';
    tiers.forEach((tier, index) => {
        const row = document.createElement('div');
        row.className = 'tier-list-row';
        row.innerHTML = `
            <div class="tier-height-pill">${escHtml(tier.height)}p</div>
            <div class="tier-list-main">
                <div class="tier-list-title">${escHtml(tier.bitrate)}</div>
                <div class="tier-list-meta">${tierRole(wrapper.dataset.key)}</div>
            </div>
            <div class="tier-list-actions">
                <button class="action-btn" type="button">Edit</button>
                <button class="action-btn danger" type="button">Delete</button>
            </div>
        `;
        row.querySelector('.tier-list-actions button:first-child').onclick = () => openTierModal(wrapper, hiddenInput, index);
        row.querySelector('.tier-list-actions button:last-child').onclick = () => {
            const next = getTierData(wrapper);
            next.splice(index, 1);
            setTierData(wrapper, hiddenInput, next);
        };
        list.appendChild(row);
    });
    wrapper.insertBefore(list, addBtn || null);
}

function openTierModal(wrapper, hiddenInput, index) {
    const tiers = getTierData(wrapper);
    const tier = index === null ? { height: '', bitrate: '' } : tiers[index];
    _tierModalState = { wrapper, hiddenInput, index };
    document.getElementById('tierModalTitle').textContent = index === null ? 'Add ABR tier' : 'Edit ABR tier';
    document.getElementById('tierHeightInput').value = tier?.height || '';
    document.getElementById('tierBitrateInput').value = tier?.bitrate || '';
    document.getElementById('tierModalStatus').textContent = '';
    document.getElementById('tierModalStatus').className = 'settings-status';
    document.getElementById('tierModal').classList.add('active');
    document.getElementById('tierHeightInput').focus();
}

function closeTierModal() {
    document.getElementById('tierModal').classList.remove('active');
    _tierModalState = null;
}

function saveTierModal() {
    const status = document.getElementById('tierModalStatus');
    const height = parseInt(document.getElementById('tierHeightInput').value, 10);
    const bitrate = document.getElementById('tierBitrateInput').value.trim();
    if (!height || height < 1 || !bitrate) {
        status.textContent = 'Height and bitrate are required.';
        status.className = 'settings-status error';
        return;
    }
    if (!_tierModalState) return;
    const tiers = getTierData(_tierModalState.wrapper);
    const nextTier = { height, bitrate };
    if (_tierModalState.index === null) tiers.push(nextTier);
    else tiers[_tierModalState.index] = nextTier;
    setTierData(_tierModalState.wrapper, _tierModalState.hiddenInput, tiers);
    closeTierModal();
}

function handleTierModalOverlayClick(e) {
    if (e.target === document.getElementById('tierModal')) closeTierModal();
}

function collectCategoryValues(catKey) {
    const result = {};
    const cat = _settingsData?.categories?.[catKey];
    if (!cat) return {};
    for (const setting of cat.settings) {
        const el = document.getElementById('sf_' + setting.key);
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

function saveSection(catKeys, btn, statusEl) {
    const values = {};
    for (const catKey of catKeys) Object.assign(values, collectCategoryValues(catKey));
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
    if (!confirm('Reset "' + key + '" to its default value?')) return;
    fetch('/api/settings/reset', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ keys: [key] }),
    }).then(async r => {
        const d = await r.json();
        if (!r.ok) throw new Error(d.error || 'Reset failed');
        _settingsData = d;
        renderAllSettings(d);
    }).catch(e => alert('Reset failed: ' + e.message));
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
        a.download = 'streamer-export.db';
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
        const uploaded = Array.isArray(d.uploads) ? d.uploads.length : 0;
        const failed = Array.isArray(d.failed_bots) ? d.failed_bots.length : 0;
        setDbExportStatus(`Uploaded snapshot to ${uploaded} bot file(s)${failed ? `; ${failed} failed` : ''}.`, failed ? 'error' : 'ok');
    }).catch(e => setDbExportStatus(e.message, 'error'));
}

function importDbExportFile() {
    const file = document.getElementById('dbImportFileInput')?.files?.[0];
    if (!file) {
        setDbImportStatus('Choose a local database file first.', 'error');
        return;
    }
    const formData = new FormData();
    formData.append('database', file);
    setDbImportStatus('Importing…');
    fetch('/api/db/import', { method: 'POST', body: formData })
        .then(async r => {
            const d = await r.json().catch(() => ({}));
            if (!r.ok) throw new Error(d.message || d.error || 'Import failed');
            setDbImportStatus(`Imported ${d.merged_jobs} jobs, ${d.merged_segments} segments, and ${d.merged_segment_parts || 0} segment parts.`, 'ok');
        })
        .catch(e => setDbImportStatus(e.message, 'error'));
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
