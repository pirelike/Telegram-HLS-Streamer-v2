// ─── Constants ────────────────────────────────────────────────────────────────
let CHUNK_SIZE = 10 * 1024 * 1024;
const MAX_RETRIES = 5;
const UPLOAD_CONCURRENCY = 3;
const ALLOWED_EXTENSIONS = new Set(['mp4', 'mkv', 'avi', 'mov', 'webm', 'ts', 'm4v', 'flv']);
const PENDING_UPLOAD_KEY = 'hls_pending_upload';

// ─── State ────────────────────────────────────────────────────────────────────
let isCancelled = false;
let currentJobId = null;
let selectedCategory = 'Film';
let pendingFiles = [];
let currentUploadController = null;

// ─── DOM refs ─────────────────────────────────────────────────────────────────
const uploadArea = document.getElementById('uploadArea');
const fileInput = document.getElementById('fileInput');
const folderInput = document.getElementById('folderInput');
const progressContainer = document.getElementById('progressContainer');
const progressBar = document.getElementById('progressBar');
const statusText = document.getElementById('statusText');
const progressStep = document.getElementById('progressStep');
const progressPct = document.getElementById('progressPct');
const speedText = document.getElementById('speedText');
const analysisCard = document.getElementById('analysisCard');
const streamBadges = document.getElementById('streamBadges');
const resultCard = document.getElementById('resultCard');
const masterUrl = document.getElementById('masterUrl');
const watchLink = document.getElementById('watchLink');
const errorMsg = document.getElementById('errorMsg');
const resumeBanner = document.getElementById('resumeBanner');
const resumeBannerText = document.getElementById('resumeBannerText');
const activityLog = document.getElementById('activityLog');

// ─── Validation ───────────────────────────────────────────────────────────────
function isValidVideoFormat(file) {
    if (file.type && file.type.startsWith('video/')) return true;
    const ext = file.name.split('.').pop().toLowerCase();
    return ALLOWED_EXTENSIONS.has(ext);
}

// ─── Category → DB mapping ───────────────────────────────────────────────────
function categoryDbFields() { return CATEGORY_DB[selectedCategory] || CATEGORY_DB['Film']; }

// ─── Segmented control ────────────────────────────────────────────────────────
document.querySelectorAll('.seg-btn').forEach(btn => {
    btn.addEventListener('click', () => {
        document.querySelectorAll('.seg-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        selectedCategory = btn.dataset.cat;
        pendingFiles = [];
        document.getElementById('metadataSection').classList.add('hidden');
        fileInput.value = '';
        folderInput.value = '';
        updateUploadUIForCategory();
    });
});

function updateUploadUIForCategory() {
    const cat = selectedCategory;
    const dropText = document.getElementById('dropText');
    const folderBtn = document.getElementById('folderUploadBtn');
    const applyAllRow = document.getElementById('applyAllRow');
    const applyAllLabel = document.getElementById('applyAllLabel');
    const isSeries = ['Film Series', 'TV Series', 'Anime TV Series'].includes(cat);
    const isFolder = ['Film Series', 'TV Series', 'Anime TV', 'Anime TV Series'].includes(cat);
    const isSingle = ['Film', 'Anime Film'].includes(cat);

    if (isSingle) {
        dropText.innerHTML = 'Drop a video file here or <strong>click to browse</strong><br><small>Supports 50GB+ files &bull; MKV, MP4, AVI, MOV, WebM &bull; Resumable</small>';
        fileInput.removeAttribute('multiple');
    } else if (cat === 'Film Series') {
        dropText.innerHTML = 'Drop a folder or multiple files or <strong>click to browse</strong><br><small>Each file is uploaded as an independent job</small>';
        fileInput.setAttribute('multiple', '');
    } else {
        dropText.innerHTML = 'Drop a season folder or <strong>single episodes</strong> here (or <strong>click to browse</strong>)<br><small>Files will be sorted by episode number</small>';
        fileInput.setAttribute('multiple', '');
    }
    folderBtn.classList.toggle('hidden', isSingle);
    applyAllRow.classList.toggle('hidden', !isSeries);
    if (isSeries) {
        applyAllLabel.textContent = cat === 'Film Series' ? 'Series name:' : 'Series name:';
    }
}
updateUploadUIForCategory();

// ─── Drag & drop with folder support ──────────────────────────────────────────
uploadArea.addEventListener('click', () => fileInput.click());
uploadArea.addEventListener('dragover', e => { e.preventDefault(); uploadArea.classList.add('dragover'); });
uploadArea.addEventListener('dragleave', () => uploadArea.classList.remove('dragover'));
uploadArea.addEventListener('drop', async e => {
    e.preventDefault(); uploadArea.classList.remove('dragover');
    const items = e.dataTransfer.items;
    let files = [];
    if (items && items.length) {
        const entries = Array.from(items).map(i => i.webkitGetAsEntry ? i.webkitGetAsEntry() : null).filter(Boolean);
        files = await readEntries(entries);
    }
    if (!files.length) files = Array.from(e.dataTransfer.files);
    files = files.filter(isValidVideoFormat);
    if (!files.length) return;
    handleSelectedFiles(files);
});

async function readEntries(entries) {
    const files = [];
    for (const entry of entries) {
        if (entry.isFile) {
            files.push(await new Promise(res => entry.file(res)));
        } else if (entry.isDirectory) {
            const reader = entry.createReader();
            const subEntries = await new Promise(res => reader.readEntries(res));
            files.push(...await readEntries(subEntries));
        }
    }
    return files;
}

fileInput.addEventListener('change', () => {
    const files = Array.from(fileInput.files).filter(isValidVideoFormat);
    fileInput.value = '';
    if (files.length) handleSelectedFiles(files);
});
folderInput.addEventListener('change', () => {
    const files = Array.from(folderInput.files).filter(isValidVideoFormat);
    folderInput.value = '';
    if (files.length) handleSelectedFiles(files);
});

// ─── File selection → metadata table ─────────────────────────────────────────
function handleSelectedFiles(files) {
    const cat = selectedCategory;
    const isSingle = ['Film', 'Anime Film'].includes(cat);
    if (isSingle) files = [files[0]];
    else files = sortByEpisode(files);
    pendingFiles = files.map(f => ({ file: f, metadata: parseMetadata(f, cat) }));
    rebuildMetadataTable();
    document.getElementById('metadataSection').classList.remove('hidden');
    validateMetadata();
}

// ─── Filename parsing ─────────────────────────────────────────────────────────
function parseMetadata(file, cat) {
    const name = file.name;
    const folder = (file.webkitRelativePath || '').split('/')[0] || '';
    if (['Film', 'Anime Film'].includes(cat)) {
        return { title: parseFilmTitle(name) };
    }
    if (cat === 'Film Series') {
        return { series_name: folder || '', title: parseFilmTitle(name), part_number: parsePartNumber(name) };
    }
    if (['TV Series', 'Anime TV Series'].includes(cat)) {
        const ep = parseSeriesEpisode(name, folder);
        return { series_name: ep.series || folder || '', season: ep.season, episode: ep.episode };
    }
    if (cat === 'Anime TV') {
        const ep = parseSeriesEpisode(name, folder);
        return { series_name: ep.series || folder || '', season: ep.season, episode: ep.episode };
    }
    return { title: parseFilmTitle(name) };
}

function parseFilmTitle(filename) {
    let s = filename.replace(/\.[^.]+$/, '');
    s = s.replace(/[\[\(][^\]\)]*[\]\)]/g, '');
    s = s.replace(/\b(19|20)\d{2}\b.*/, '');
    s = s.replace(/\b(1080p|720p|480p|4k|bluray|bdrip|webrip|web-dl|hdtv|x264|x265|hevc|avc|remux|proper)\b.*/i, '');
    s = s.replace(/[._-]+/g, ' ').trim();
    return s.replace(/\s+/g, ' ').trim();
}

function parseSeriesEpisode(filename, folder) {
    let m;
    let season = null, episode = null, series = folder || '';
    if ((m = filename.match(/[Ss](\d+)[Ee](\d+)/))) {
        season = parseInt(m[1], 10); episode = parseInt(m[2], 10);
    } else if ((m = filename.match(/[Ss]eason\s*(\d+)[^0-9]*[Ee]p?(?:isode)?\s*(\d+)/i))) {
        season = parseInt(m[1], 10); episode = parseInt(m[2], 10);
    } else if ((m = filename.match(/[Ee]p?(\d+)/i))) {
        episode = parseInt(m[1], 10);
    }
    return { series, season, episode };
}

function parsePartNumber(filename) {
    let m;
    if ((m = filename.match(/[Pp]art\s*(\d+)/i))) return parseInt(m[1], 10);
    if ((m = filename.match(/[Pp]t\.?\s*(\d+)/i))) return parseInt(m[1], 10);
    return null;
}

// ─── Metadata table builder ───────────────────────────────────────────────────
function rebuildMetadataTable() {
    const cat = selectedCategory;
    const wrap = document.getElementById('metadataTableWrap');

    let cols = [];
    if (cat === 'Film' || cat === 'Anime Film') {
        cols = [{key:'title', label:'Movie Title', required:true}];
    } else if (cat === 'Film Series') {
        cols = [
            {key:'series_name', label:'Series Name', required:true},
            {key:'title', label:'Film Title', required:true},
            {key:'part_number', label:'Part #', required:true, type:'number'},
        ];
    } else if (['TV Series', 'Anime TV', 'Anime TV Series'].includes(cat)) {
        cols = [
            {key:'series_name', label:'Series', required:true},
            {key:'season', label:'Season', required:false, type:'number'},
            {key:'episode', label:'Episode', required:true, type:'number'},
        ];
    }

    let html = '<table class="metadata-table"><thead><tr><th>File</th>';
    for (const c of cols) html += `<th>${c.label}</th>`;
    html += '<th>Metadata</th>';
    if (pendingFiles.length > 1) html += '<th>Status</th>';
    html += '</tr></thead><tbody>';

    pendingFiles.forEach((pf, i) => {
        html += `<tr data-idx="${i}"><td><span class="meta-filename" title="${escapeAttr(pf.file.name)}">${escapeHtml(pf.file.name)}</span></td>`;
        for (const c of cols) {
            const val = pf.metadata[c.key] != null ? pf.metadata[c.key] : '';
            html += `<td><input class="meta-input" type="${c.type||'text'}" data-key="${c.key}" data-idx="${i}" value="${escapeAttr(String(val))}" placeholder="${c.label}"></td>`;
        }
        const provider = selectedCategory.startsWith('Anime') ? 'anilist' : 'tmdb';
        html += `<td class="upload-meta-link-cell">
            <div style="display:flex;gap:6px;align-items:center;min-width:260px;">
                <select class="form-input upload-meta-provider" data-idx="${i}" style="width:88px;">
                    <option value="tmdb"${provider === 'tmdb' ? ' selected' : ''}>TMDB</option>
                    <option value="anilist"${provider === 'anilist' ? ' selected' : ''}>AniList</option>
                </select>
                <input class="form-input upload-meta-query" data-idx="${i}" type="text" value="${escapeAttr(defaultMetadataQuery(pf.metadata))}" style="min-width:120px;" placeholder="Search">
                <button type="button" class="action-btn" onclick="searchUploadMetadata(${i})">Search</button>
            </div>
            <div class="upload-meta-results" id="upload-meta-results-${i}" style="margin-top:6px;"></div>
        </td>`;
        if (pendingFiles.length > 1) html += `<td><span class="file-row-status" id="row-status-${i}"></span></td>`;
        html += '</tr>';
    });
    html += '</tbody></table>';
    wrap.innerHTML = html;

    wrap.addEventListener('input', e => {
        const input = e.target;
        if (!input.classList.contains('meta-input')) return;
        const idx = parseInt(input.dataset.idx, 10);
        const key = input.dataset.key;
        const val = input.value;
        pendingFiles[idx].metadata[key] = input.type === 'number' ? (val === '' ? null : parseInt(val, 10)) : val;
        validateMetadata();
    });
}

function defaultMetadataQuery(metadata) {
    return metadata.series_name || metadata.title || '';
}

async function searchUploadMetadata(idx) {
    const pf = pendingFiles[idx];
    if (!pf) return;
    const providerEl = document.querySelector(`.upload-meta-provider[data-idx="${idx}"]`);
    const queryEl = document.querySelector(`.upload-meta-query[data-idx="${idx}"]`);
    const resultsEl = document.getElementById(`upload-meta-results-${idx}`);
    const provider = providerEl ? providerEl.value : 'tmdb';
    const q = queryEl ? queryEl.value.trim() : '';
    if (!q || !resultsEl) return;
    resultsEl.innerHTML = '<span style="font-size:12px;color:var(--t-ink-3);">Searching...</span>';
    try {
        const resp = await fetch(`/api/metadata/search?provider=${encodeURIComponent(provider)}&q=${encodeURIComponent(q)}`);
        const data = await resp.json();
        const items = (data.results || []).slice(0, 5);
        if (!resp.ok) throw new Error(data.message || data.error || 'Search failed');
        if (!items.length) {
            resultsEl.innerHTML = '<span style="font-size:12px;color:var(--t-ink-3);">No results</span>';
            return;
        }
        resultsEl.innerHTML = items.map((item, itemIdx) => {
            const title = escapeHtml(item.title || item.original_title || 'Untitled');
            const year = item.year ? ` (${item.year})` : '';
            return `<button type="button" class="action-btn" style="margin:0 4px 4px 0;padding:5px 8px;font-size:12px;" onclick="selectUploadMetadata(${idx}, ${itemIdx})">${title}${escapeHtml(year)}</button>`;
        }).join('');
        pf.metadata._metadata_results = items;
    } catch (e) {
        resultsEl.innerHTML = `<span style="font-size:12px;color:#ff6b6b;">${escapeHtml(e.message || 'Search failed')}</span>`;
    }
}

function selectUploadMetadata(idx, itemIdx) {
    const pf = pendingFiles[idx];
    if (!pf || !pf.metadata._metadata_results) return;
    const item = pf.metadata._metadata_results[itemIdx];
    if (!item) return;
    pf.metadata.external_metadata = {
        provider: item.provider,
        provider_id: item.provider_id,
        media_kind: item.media_kind,
    };
    const resultsEl = document.getElementById(`upload-meta-results-${idx}`);
    if (resultsEl) {
        const title = escapeHtml(item.title || item.original_title || item.provider_id);
        resultsEl.innerHTML = `<span style="font-size:12px;color:var(--t-accent);">Linked: ${title}</span>`;
    }
}

// ─── Validation ───────────────────────────────────────────────────────────────
function validateMetadata() {
    if (!pendingFiles.length) { document.getElementById('startUploadBtn').disabled = true; return; }
    const cat = selectedCategory;
    let valid = true;
    for (const pf of pendingFiles) {
        const m = pf.metadata;
        if (['Film', 'Anime Film'].includes(cat) && !(m.title && m.title.trim())) { valid = false; break; }
        if (cat === 'Film Series' && (!(m.series_name && m.series_name.trim()) || !(m.title && m.title.trim()) || m.part_number == null)) { valid = false; break; }
        if (['TV Series', 'Anime TV', 'Anime TV Series'].includes(cat) && (!(m.series_name && m.series_name.trim()) || m.episode == null)) { valid = false; break; }
    }
    document.getElementById('startUploadBtn').disabled = !valid;
}

// ─── Apply to all ─────────────────────────────────────────────────────────────
document.getElementById('applyAllBtn').addEventListener('click', () => {
    const val = document.getElementById('applyAllInput').value.trim();
    if (!val) return;
    pendingFiles.forEach((pf, i) => {
        pf.metadata.series_name = val;
        const inp = document.querySelector(`.meta-input[data-key="series_name"][data-idx="${i}"]`);
        if (inp) inp.value = val;
    });
    validateMetadata();
});

// ─── Start Upload orchestrator ────────────────────────────────────────────────
document.getElementById('startUploadBtn').addEventListener('click', async () => {
    if (!pendingFiles.length) return;
    document.getElementById('startUploadBtn').disabled = true;
    document.getElementById('metadataSection').classList.add('hidden');
    uploadArea.classList.add('disabled');
    isCancelled = false;
    errorMsg.classList.add('hidden');
    resultCard.classList.add('hidden');
    analysisCard.classList.add('hidden');
    resumeBanner.classList.add('hidden');
    progressContainer.classList.remove('hidden');
    clearActivityLog();
    addActivity('Upload started');

    const dbFields = categoryDbFields();
    let lastJobId = null;
    for (let i = 0; i < pendingFiles.length; i++) {
        if (isCancelled) break;
        const pf = pendingFiles[i];
        progressStep.textContent = `File ${i + 1} of ${pendingFiles.length}: ${pf.file.name}`;
        addActivity(`Selected ${pf.file.name} (${formatBytes(pf.file.size)})`);
        setRowStatus(i, 'uploading', 'Uploading…');
        try {
            const jobId = await uploadSingleFile(pf.file, pf.metadata, dbFields);
            setRowStatus(i, 'complete', '✓');
            lastJobId = jobId;
        } catch (err) {
            setRowStatus(i, 'error', '✗');
            showError(`Stopped at "${pf.file.name}": ${err.message}`);
            uploadArea.classList.remove('disabled');
            document.getElementById('metadataSection').classList.remove('hidden');
            document.getElementById('startUploadBtn').disabled = false;
            return;
        }
    }

    uploadArea.classList.remove('disabled');
    document.getElementById('metadataSection').classList.remove('hidden');
    document.getElementById('startUploadBtn').disabled = false;
    const uploadCount = pendingFiles.length;
    pendingFiles = [];
    if (!isCancelled) {
        statusText.textContent = `Done — ${uploadCount} file${uploadCount !== 1 ? 's' : ''} uploaded.`;
        if (lastJobId) showResultCard(lastJobId);
    }
});

function setRowStatus(idx, cls, text) {
    const el = document.getElementById(`row-status-${idx}`);
    if (!el) return;
    el.className = `file-row-status ${cls}`;
    el.textContent = text;
}

function clearActivityLog() {
    if (activityLog) activityLog.innerHTML = '';
}

function addActivity(message) {
    if (!activityLog) return;
    const time = new Date().toLocaleTimeString([], {hour: '2-digit', minute: '2-digit', second: '2-digit'});
    const row = document.createElement('div');
    row.className = 'activity-log-row';
    row.innerHTML = `<span class="activity-log-time">${escapeHtml(time)}</span><i class="material-icons-round">radio_button_checked</i><span>${escapeHtml(message)}</span>`;
    activityLog.prepend(row);
    while (activityLog.children.length > 8) activityLog.removeChild(activityLog.lastChild);
}

// ─── Resume helpers ───────────────────────────────────────────────────────────
function savePendingUpload(uploadId, filename, fileSize, chunkSize, totalChunks, nextChunk) {
    localStorage.setItem(PENDING_UPLOAD_KEY, JSON.stringify(
        {uploadId, filename, fileSize, chunkSize, totalChunks, nextChunk, timestamp: Date.now()}
    ));
}
function getPendingUpload() {
    try { const r = localStorage.getItem(PENDING_UPLOAD_KEY); return r ? JSON.parse(r) : null; }
    catch { return null; }
}
function clearPendingUpload() { localStorage.removeItem(PENDING_UPLOAD_KEY); }
function dismissResume() { clearPendingUpload(); resumeBanner.classList.add('hidden'); }

function checkResume() {
    const pending = getPendingUpload();
    if (!pending) { resumeBanner.classList.add('hidden'); return; }
    const age = Math.round((Date.now() - (pending.timestamp || Date.now())) / 1000);
    let ageStr = age < 60 ? 'just now' : `${Math.round(age/60)}m ago`;
    if (age > 3600) ageStr = `${Math.round(age/3600)}h ago`;
    if (age > 86400) ageStr = `${Math.round(age/86400)}d ago`;
    const pct = pending.totalChunks > 0 ? Math.round(pending.nextChunk / pending.totalChunks * 100) : 0;
    resumeBannerText.textContent =
        `Incomplete upload: "${pending.filename}" (~${pct}%). Last activity: ${ageStr}. Re-select the same file to resume.`;
    resumeBanner.classList.remove('hidden');
}

// ─── Cancel ───────────────────────────────────────────────────────────────────
function cancelUpload() {
    isCancelled = true;
    if (currentUploadController) {
        currentUploadController.abort();
        currentUploadController = null;
    }
    clearPendingUpload();
    if (currentJobId) {
        fetch(`/api/cancel/${currentJobId}`, {method:'POST'}).catch(()=>{});
        currentJobId = null;
    }
    uploadArea.classList.remove('disabled');
    document.getElementById('metadataSection').classList.remove('hidden');
    document.getElementById('startUploadBtn').disabled = false;
}

// ─── Episode sort ─────────────────────────────────────────────────────────────
function sortByEpisode(files) {
    function episodeNum(name) {
        let m;
        if ((m = name.match(/[Ss]\d+[Ee](\d+)/))) return parseInt(m[1], 10) * 1000 + parseInt(name.match(/[Ss](\d+)/)[1], 10);
        if ((m = name.match(/[Ee][Pp]?(\d{1,3})(?!\d)/))) return parseInt(m[1], 10);
        if ((m = name.match(/\d+x(\d+)/))) return parseInt(m[1], 10);
        if ((m = name.match(/(?:^|[\s\-_\.])(\d{1,3})(?:[\s\-_\.]|$)/))) return parseInt(m[1], 10);
        return Infinity;
    }
    return [...files].sort((a, b) => {
        const na = episodeNum(a.name), nb = episodeNum(b.name);
        if (na !== nb) return na - nb;
        return a.name.localeCompare(b.name);
    });
}

// ─── Upload single file ────────────────────────────────────────────────────────
async function uploadSingleFile(file, metadata, dbFields) {
    if (!isValidVideoFormat(file)) throw new Error(`Unsupported format: "${file.name}"`);

    progressBar.style.width = '0%';
    progressPct.textContent = '0%';
    speedText.textContent = '';
    currentJobId = null;

    let uploadId = null, totalChunks = 0;
    let receivedIndices = new Set();
    const pending = getPendingUpload();
    if (pending && pending.filename === file.name && pending.fileSize === file.size) {
        try {
            const s = await fetch(`/api/upload/status/${pending.uploadId}`);
            if (s.ok) {
                const d = await s.json();
                CHUNK_SIZE = Number(d.chunk_size || pending.chunkSize || CHUNK_SIZE);
                totalChunks = Number(d.total_chunks || pending.totalChunks || 0);
                receivedIndices = receivedSetFromStatus(d, totalChunks, pending.nextChunk);
                uploadId = pending.uploadId;
                statusText.textContent = `Resuming "${file.name}" (${receivedIndices.size}/${totalChunks} chunks already uploaded)...`;
            } else { clearPendingUpload(); }
        } catch { clearPendingUpload(); }
    }

    if (!uploadId) {
        statusText.textContent = `Preparing ${file.name} (${formatBytes(file.size)})...`;
        addActivity('Requesting upload slot');
        const initResp = await fetch('/api/upload/init', {
            method: 'POST', headers: {'Content-Type':'application/json'},
            body: JSON.stringify({filename: file.name, total_size: file.size}),
        });
        if (!initResp.ok) { const e = await initResp.json(); throw new Error(e.message || e.error || 'Init failed'); }
        const init = await initResp.json();
        uploadId = init.upload_id;
        CHUNK_SIZE = Number(init.chunk_size);
        totalChunks = Number(init.total_chunks);
        receivedIndices = new Set();
        addActivity(`Upload slot ready: ${totalChunks} chunks of ${formatBytes(CHUNK_SIZE)}`);
    }

    currentUploadController = new AbortController();
    const missingChunks = [];
    for (let i = 0; i < totalChunks; i++) {
        if (!receivedIndices.has(i)) missingChunks.push(i);
    }

    let uploadedSinceStart = 0;
    const startTime = Date.now();
    let uploadedBytes = bytesForReceivedChunks(file.size, totalChunks, CHUNK_SIZE, receivedIndices);
    updateUploadProgress(file, uploadedBytes, receivedIndices.size, totalChunks, startTime, uploadedSinceStart);
    let nextWork = 0;
    const workerCount = Math.min(UPLOAD_CONCURRENCY, missingChunks.length);
    async function worker() {
        while (nextWork < missingChunks.length) {
            if (isCancelled) throw new Error('Upload cancelled.');
            const i = missingChunks[nextWork++];
            const start = i * CHUNK_SIZE, end = Math.min(start + CHUNK_SIZE, file.size);
            await sendChunkWithRetry(uploadId, i, file.slice(start, end), currentUploadController.signal);
            if (isCancelled) throw new Error('Upload cancelled.');
            if (!receivedIndices.has(i)) {
                receivedIndices.add(i);
                const chunkBytes = end - start;
                uploadedBytes += chunkBytes;
                uploadedSinceStart += chunkBytes;
            }
            savePendingUpload(uploadId, file.name, file.size, CHUNK_SIZE, totalChunks, receivedIndices.size);
            updateUploadProgress(file, uploadedBytes, receivedIndices.size, totalChunks, startTime, uploadedSinceStart);
            if (receivedIndices.size === totalChunks || receivedIndices.size % 5 === 0) {
                addActivity(`Uploaded ${receivedIndices.size} of ${totalChunks} chunks`);
            }
        }
    }
    try {
        await Promise.all(Array.from({length: workerCount}, () => worker()));
    } catch (err) {
        if (currentUploadController) currentUploadController.abort();
        throw err;
    } finally {
        currentUploadController = null;
    }

    if (isCancelled) throw new Error('Upload cancelled.');

    statusText.textContent = 'Finalizing upload...';
    addActivity('Finalizing upload and queueing job');
    const finalBody = {
        upload_id: uploadId,
        metadata: {
            media_type: dbFields.media_type,
            title: metadata.title || null,
            series_name: metadata.series_name || '',
            is_series: !!dbFields.is_series,
            season_number: metadata.season != null ? metadata.season : null,
            episode_number: metadata.episode != null ? metadata.episode : null,
            part_number: metadata.part_number != null ? metadata.part_number : null,
        },
    };
    const finalResp = await fetch('/api/upload/finalize', {
        method: 'POST', headers: {'Content-Type':'application/json'},
        body: JSON.stringify(finalBody),
    });
    if (!finalResp.ok) { const e = await finalResp.json(); throw new Error(e.message || e.error || 'Finalize failed'); }
    const {job_id} = await finalResp.json();
    currentJobId = job_id;
    clearPendingUpload();
    if (metadata.external_metadata) {
        await linkUploadedMetadata(job_id, metadata.external_metadata);
    }

    statusText.textContent = 'Processing...';
    addActivity(`Server job queued: ${job_id}`);
    return pollStatus(job_id);
}

async function linkUploadedMetadata(jobId, externalMetadata) {
    try {
        const resp = await fetch(`/api/jobs/${encodeURIComponent(jobId)}/metadata/link`, {
            method: 'POST',
            headers: {'Content-Type':'application/json'},
            body: JSON.stringify({
                provider: externalMetadata.provider,
                provider_id: externalMetadata.provider_id,
                media_kind: externalMetadata.media_kind,
            }),
        });
        if (resp.ok) addActivity('External metadata linked');
    } catch {}
}

function receivedSetFromStatus(status, totalChunks, fallbackNextChunk) {
    if (Array.isArray(status.received_indices)) {
        return new Set(status.received_indices.map(Number).filter(i => Number.isInteger(i) && i >= 0 && i < totalChunks));
    }
    const contiguous = Number(status.received_chunks || fallbackNextChunk || 0);
    return new Set(Array.from({length: Math.min(contiguous, totalChunks)}, (_, i) => i));
}

function bytesForReceivedChunks(fileSize, totalChunks, chunkSize, receivedIndices) {
    let bytes = 0;
    for (const i of receivedIndices) {
        const start = i * chunkSize, end = Math.min(start + chunkSize, fileSize);
        if (i >= 0 && i < totalChunks && end > start) bytes += end - start;
    }
    return bytes;
}

function updateUploadProgress(file, uploadedBytes, receivedCount, totalChunks, startTime, uploadedSinceStart) {
    const pct = file.size > 0 ? Math.round(uploadedBytes / file.size * 100) : 100;
    progressBar.style.width = pct + '%';
    progressPct.textContent = pct + '%';
    statusText.textContent = `Uploading ${file.name}... (${formatBytes(uploadedBytes)} / ${formatBytes(file.size)})`;
    progressStep.textContent = `Chunks ${receivedCount} / ${totalChunks}`;
    const elapsed = (Date.now() - startTime) / 1000;
    if (elapsed > 0 && uploadedSinceStart > 0) {
        const speed = uploadedSinceStart / elapsed;
        speedText.textContent = `${formatBytes(speed)}/s — ~${formatTime((file.size - uploadedBytes) / speed)} remaining`;
    }
}

async function sendChunkWithRetry(uploadId, chunkIndex, chunkBlob, signal) {
    for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
        if (isCancelled) throw new Error('Upload cancelled.');
        try {
            const resp = await fetch('/api/upload/chunk', {
                method: 'POST',
                headers: {'X-Upload-Id': uploadId, 'X-Chunk-Index': chunkIndex.toString(), 'Content-Type': 'application/octet-stream'},
                body: chunkBlob,
                signal,
            });
            if (resp.ok) return;
            if (resp.status === 429) {
                const wait = 60_000;
                speedText.textContent = `Rate limited, waiting ${wait/1000}s for window to reset...`;
                await new Promise(r => setTimeout(r, wait));
                attempt--;
                continue;
            }
            const e = await resp.json();
            throw new Error(e.message || e.error || `HTTP ${resp.status}`);
        } catch (err) {
            if (isCancelled || err.name === 'AbortError') throw new Error('Upload cancelled.');
            if (attempt < MAX_RETRIES - 1) {
                const wait = Math.pow(2, attempt) * 1000;
                speedText.textContent = `Chunk ${chunkIndex} failed, retrying in ${wait/1000}s... (${err.message})`;
                await new Promise(r => setTimeout(r, wait));
            } else { throw new Error(`Chunk ${chunkIndex} failed after ${MAX_RETRIES} retries: ${err.message}`); }
        }
    }
}

function pollStatus(jobId) {
    return new Promise((resolve, reject) => {
        let lastStatus = '';
        let lastDescription = '';
        const interval = setInterval(() => {
            fetch(`/api/status/${jobId}`).then(r => r.json()).then(data => {
                if (data.analysis && !analysisCard.classList.contains('active')) showAnalysis(data.analysis);
                let label;
                if (data.status === 'queued') {
                    const pos = data.queue_position;
                    const depth = data.queue_depth;
                    label = pos != null && depth != null
                        ? `Queued... (position ${pos} of ${depth})`
                        : pos != null ? `Queued... (position ${pos})` : 'Queued for processing...';
                } else if (data.status === 'analyzing') { label = data.description || 'Analyzing video streams...'; }
                else if (data.status === 'processing') { label = data.description || 'Processing video, audio, subtitles, and thumbnail...'; }
                else if (data.status === 'uploading') {
                    label = data.description || 'Uploading HLS output to Telegram...';
                } else if (data.status === 'complete') { label = 'Complete!'; }
                else if (data.status === 'error') { label = 'Error: ' + (data.error || 'Unknown'); }
                else { label = data.status; }
                statusText.textContent = label;
                if (data.step) progressStep.textContent = `Step ${data.step} of ${data.total_steps || 5}`;
                if (data.progress !== undefined) {
                    const pct = Math.round(Number(data.progress));
                    progressBar.style.width = pct + '%';
                    progressPct.textContent = pct + '%';
                }
                speedText.textContent = '';
                if (data.status !== lastStatus || (data.description || '') !== lastDescription) {
                    addActivity(label);
                    lastStatus = data.status;
                    lastDescription = data.description || '';
                }
                if (data.status === 'complete') {
                    clearInterval(interval);
                    currentJobId = null;
                    resolve(jobId);
                } else if (data.status === 'error') {
                    clearInterval(interval);
                    currentJobId = null;
                    reject(new Error(data.error || 'Processing failed'));
                }
            }).catch(()=>{});
        }, 1500);
    });
}

function showAnalysis(analysis) {
    analysisCard.classList.remove('hidden');
    streamBadges.innerHTML = '';
    if (analysis.video_tracks > 0) streamBadges.innerHTML += `<span class="badge badge-video"><i class="material-icons-round">videocam</i> Video: ${analysis.video_tracks} track(s)</span>`;
    if (analysis.audio_tracks > 0) streamBadges.innerHTML += `<span class="badge badge-audio"><i class="material-icons-round">audiotrack</i> Audio: ${analysis.audio_tracks} track(s)</span>`;
    if (analysis.subtitle_tracks > 0) streamBadges.innerHTML += `<span class="badge badge-sub"><i class="material-icons-round">subtitles</i> Subtitles: ${analysis.subtitle_tracks} track(s)</span>`;
}

function showResultCard(jobId) {
    const url = `${window.location.origin}/hls/${jobId}/master.m3u8`;
    masterUrl.textContent = url;
    if (watchLink) watchLink.href = `/watch/${jobId}`;
    resultCard.classList.remove('hidden');
}

function showError(msg) {
    errorMsg.textContent = msg;
    errorMsg.classList.remove('hidden');
    progressContainer.classList.add('hidden');
}

function copyUrl() {
    navigator.clipboard.writeText(masterUrl.textContent).then(() => {
        const btn = document.querySelector('.copy-btn');
        btn.textContent = 'Copied!';
        setTimeout(() => btn.textContent = 'Copy', 2000);
    });
}

// ─── Init ─────────────────────────────────────────────────────────────────────
checkResume();
