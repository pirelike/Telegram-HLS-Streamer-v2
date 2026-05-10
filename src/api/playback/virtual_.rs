use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use axum::http::StatusCode;
use axum::response::Response;
use tokio::process::Command;

use super::cache::{claim_inflight, finish_inflight, CacheEntry};
use super::{api_error, AppState};
use crate::config::Config;
use crate::{db, media};

pub(super) fn is_virtual_key(key: &str) -> bool {
    if let Some(prefix) = key.split('/').next() {
        if let Some(rest) = prefix.strip_prefix("virtual_") {
            return rest.ends_with('p');
        }
    }
    false
}

pub(super) async fn serve_virtual_segment(
    state: Arc<AppState>,
    job_id: String,
    key: String,
) -> Response {
    let cache_key = format!("{job_id}/{key}");
    if let Some(entry) = state.cache.get(&cache_key).await {
        return super::cache_response(entry);
    }
    state
        .cache
        .misses
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let entry = match fetch_virtual_with_singleflight(&state, &job_id, &key).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, key = %key, "virtual segment fetch failed");
            return api_error(StatusCode::NOT_FOUND, "fetch_failed", e.to_string());
        }
    };
    spawn_prefetch_virtual(state.clone(), job_id, key);
    super::cache_response(entry)
}

async fn fetch_virtual_with_singleflight(
    state: &Arc<AppState>,
    job_id: &str,
    key: &str,
) -> Result<CacheEntry> {
    let cache_key = format!("{job_id}/{key}");
    let (inflight, is_leader) = claim_inflight(state, &cache_key).await;
    if !is_leader {
        inflight.notify.notified().await;
        let outcome = inflight.outcome.lock().await.clone();
        match outcome {
            Some(Ok(())) => {
                if let Some(entry) = state.cache.get(&cache_key).await {
                    return Ok(entry);
                }
                bail!("cache miss after single-flight wait");
            }
            Some(Err(e)) => bail!(e),
            None => bail!("leader produced no outcome"),
        }
    }
    let result = virtual_fetch_into_cache(state, job_id, key).await;
    finish_inflight(state, &cache_key, inflight, &result).await;
    result
}

async fn virtual_fetch_into_cache(
    state: &Arc<AppState>,
    job_id: &str,
    key: &str,
) -> Result<CacheEntry> {
    let cfg = state.config.read().await.clone();
    if !cfg.virtual_abr_tiers || cfg.abr_enabled {
        bail!("virtual ABR not enabled");
    }
    let (prefix, filename) = key
        .split_once('/')
        .ok_or_else(|| anyhow!("malformed virtual key"))?;
    let height_str = prefix
        .strip_prefix("virtual_")
        .and_then(|s| s.strip_suffix('p'))
        .ok_or_else(|| anyhow!("malformed virtual prefix"))?;
    let target_height: u32 = height_str.parse().context("bad height")?;
    let bitrate = media::tier_bitrate(&cfg.abr_tiers, target_height as i64)
        .ok_or_else(|| anyhow!("height {target_height} not configured"))?;
    tracing::info!(
        job_id,
        key,
        target_height,
        bitrate = %bitrate,
        "virtual abr segment requested"
    );

    if filename == "init.mp4" {
        let init_bytes = build_virtual_init(state, &cfg, job_id, target_height, &bitrate).await?;
        let entry = CacheEntry {
            content_type: super::content_type_for(filename),
            bytes: Arc::new(init_bytes),
        };
        let cache_key = format!("{job_id}/{key}");
        state
            .cache
            .insert(
                cache_key,
                entry.clone(),
                Some((cfg.segment_cache_size_mb as u64) * 1024 * 1024),
            )
            .await;
        return Ok(entry);
    }

    let source_key = format!("video_0/{filename}");
    let source_bytes = fetch_source_for_virtual(state, job_id, &source_key).await?;
    let init_source = fetch_source_for_virtual(state, job_id, "video_0/init.mp4").await?;

    let transcoded = transcode_segment(&cfg, &init_source, &source_bytes, target_height, &bitrate)
        .await
        .with_context(|| format!("transcoding virtual {key}"))?;
    let media_bytes = strip_init_from_fmp4(&transcoded)?;

    let entry = CacheEntry {
        content_type: super::content_type_for(filename),
        bytes: Arc::new(media_bytes),
    };
    let cache_key = format!("{job_id}/{key}");
    state
        .cache
        .insert(
            cache_key,
            entry.clone(),
            Some((cfg.segment_cache_size_mb as u64) * 1024 * 1024),
        )
        .await;
    Ok(entry)
}

async fn build_virtual_init(
    state: &Arc<AppState>,
    cfg: &Config,
    job_id: &str,
    target_height: u32,
    bitrate: &str,
) -> Result<Vec<u8>> {
    // Find first source media segment (lowest filename in video_0/, excluding init).
    let segs = {
        let conn = state.db.lock().await;
        db::get_segments_for_prefix(&conn, job_id, "video_0")?
    };
    let first = segs
        .iter()
        .find(|s| !s.segment_key.ends_with("/init.mp4"))
        .ok_or_else(|| anyhow!("no source media segments"))?;
    let init_source = fetch_source_for_virtual(state, job_id, "video_0/init.mp4").await?;
    let media_source = fetch_source_for_virtual(state, job_id, &first.segment_key).await?;
    let transcoded =
        transcode_segment(cfg, &init_source, &media_source, target_height, bitrate).await?;
    let init_part = extract_init_from_fmp4(&transcoded)?;
    // Also opportunistically cache the first virtual media segment we just produced.
    if let Some((_, filename)) = first.segment_key.split_once('/') {
        let media_part = strip_init_from_fmp4(&transcoded)?;
        let virt_key = format!("virtual_{target_height}p/{filename}");
        let cache_key = format!("{job_id}/{virt_key}");
        let entry = CacheEntry {
            content_type: super::content_type_for(filename),
            bytes: Arc::new(media_part),
        };
        state
            .cache
            .insert(
                cache_key,
                entry,
                Some((cfg.segment_cache_size_mb as u64) * 1024 * 1024),
            )
            .await;
    }
    Ok(init_part)
}

async fn fetch_source_for_virtual(
    state: &Arc<AppState>,
    job_id: &str,
    source_key: &str,
) -> Result<Vec<u8>> {
    let cache_key = format!("{job_id}/{source_key}");
    if let Some(entry) = state.cache.get(&cache_key).await {
        return Ok((*entry.bytes).clone());
    }
    let (file_id, bot_index) = {
        let conn = state.db.lock().await;
        db::get_segment(&conn, job_id, source_key)?
            .ok_or_else(|| anyhow!("source segment {source_key} not found"))?
            .into_tuple()
    };
    let entry = super::real::fetch_real_with_singleflight(
        state, &cache_key, &file_id, bot_index, source_key,
    )
    .await?;
    Ok((*entry.bytes).clone())
}

async fn transcode_segment(
    cfg: &Config,
    init_bytes: &[u8],
    media_bytes: &[u8],
    target_height: u32,
    bitrate: &str,
) -> Result<Vec<u8>> {
    let tmp_dir = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let in_path = tmp_dir.join(format!("thls-vabr-in-{stamp}.mp4"));
    let out_path = tmp_dir.join(format!("thls-vabr-out-{stamp}.mp4"));

    let mut combined = Vec::with_capacity(init_bytes.len() + media_bytes.len());
    combined.extend_from_slice(init_bytes);
    combined.extend_from_slice(media_bytes);
    tokio::fs::write(&in_path, &combined).await?;

    let encoder = media::select_encoder(cfg).await;
    let scale = format!("scale='trunc({target_height}*16/9/2)*2':{target_height}");
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-nostdin").arg("-loglevel").arg("error").arg("-y");
    if let Some(device) = &encoder.vaapi_device {
        cmd.arg("-vaapi_device").arg(device);
    }
    cmd.arg("-i")
        .arg(&in_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-sn")
        .arg("-c:v")
        .arg(&encoder.name)
        .arg("-b:v")
        .arg(bitrate);
    if let Some(filter) = media::video_filter(&encoder, Some(scale)) {
        cmd.arg("-vf").arg(filter);
    }
    tracing::info!(
        target_height,
        bitrate,
        encoder = %encoder.name,
        vaapi_device = encoder.vaapi_device.as_deref().unwrap_or(""),
        "virtual abr transcode started"
    );
    tracing::debug!(cmd = ?cmd, "virtual abr ffmpeg command");
    let started = std::time::Instant::now();
    let status = cmd
        .arg("-f")
        .arg("mp4")
        .arg("-movflags")
        .arg("frag_keyframe+empty_moov+default_base_moof")
        .arg(&out_path)
        .status()
        .await
        .context("running ffmpeg for virtual transcode")?;
    let _ = tokio::fs::remove_file(&in_path).await;
    if !status.success() {
        let _ = tokio::fs::remove_file(&out_path).await;
        bail!("ffmpeg virtual transcode exited with {status}");
    }
    let bytes = tokio::fs::read(&out_path).await?;
    let _ = tokio::fs::remove_file(&out_path).await;
    tracing::info!(
        target_height,
        bitrate,
        output_bytes = bytes.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "virtual abr transcode complete"
    );
    Ok(bytes)
}

// --- fMP4 split helpers ------------------------------------------------------

pub(super) fn extract_init_from_fmp4(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let boxes = scan_top_level_boxes(data)?;
    for (typ, range) in boxes {
        if typ == *b"ftyp" || typ == *b"moov" {
            out.extend_from_slice(&data[range]);
        }
    }
    if out.is_empty() {
        bail!("no ftyp/moov found in fMP4 output");
    }
    Ok(out)
}

pub(super) fn strip_init_from_fmp4(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let boxes = scan_top_level_boxes(data)?;
    for (typ, range) in boxes {
        if typ != *b"ftyp" && typ != *b"moov" {
            out.extend_from_slice(&data[range]);
        }
    }
    if out.is_empty() {
        bail!("no media boxes found in fMP4 output");
    }
    Ok(out)
}

pub(super) fn scan_top_level_boxes(data: &[u8]) -> Result<Vec<([u8; 4], std::ops::Range<usize>)>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as u64;
        let mut typ = [0u8; 4];
        typ.copy_from_slice(&data[i + 4..i + 8]);
        let (header_len, box_size) = if size == 1 {
            if i + 16 > data.len() {
                bail!("truncated 64-bit box header");
            }
            let large = u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap());
            (16usize, large)
        } else if size == 0 {
            (8usize, (data.len() - i) as u64)
        } else {
            (8usize, size)
        };
        if box_size < header_len as u64 {
            bail!("invalid box size");
        }
        let end = i + box_size as usize;
        if end > data.len() {
            bail!("box extends past data");
        }
        out.push((typ, i..end));
        i = end;
    }
    Ok(out)
}

// --- prefetch ----------------------------------------------------------------

fn spawn_prefetch_virtual(state: Arc<AppState>, job_id: String, key: String) {
    tokio::spawn(async move {
        let cfg = state.config.read().await.clone();
        let n = cfg.segment_prefetch_count as usize;
        if n == 0 {
            return;
        }
        if cfg.segment_prefetch_min_free_bytes > 0
            && state.cache.free_bytes().await < cfg.segment_prefetch_min_free_bytes
        {
            return;
        }
        let (prefix, filename) = match key.split_once('/') {
            Some(p) => p,
            None => return,
        };
        let height = match prefix
            .strip_prefix("virtual_")
            .and_then(|s| s.strip_suffix('p'))
        {
            Some(h) => h.to_string(),
            None => return,
        };
        if filename == "init.mp4" {
            return;
        }
        let segs = {
            let conn = state.db.lock().await;
            match db::get_segments_for_prefix(&conn, &job_id, "video_0") {
                Ok(s) => s,
                Err(_) => return,
            }
        };
        let media: Vec<&db::SegmentRow> = segs
            .iter()
            .filter(|s| !s.segment_key.ends_with("/init.mp4"))
            .collect();
        let pos = match media
            .iter()
            .position(|s| s.segment_key.split_once('/').map(|p| p.1) == Some(filename))
        {
            Some(p) => p,
            None => return,
        };
        for next in media.iter().skip(pos + 1).take(n) {
            let next_filename = match next.segment_key.split_once('/') {
                Some((_, f)) => f,
                None => continue,
            };
            let virt_key = format!("virtual_{height}p/{next_filename}");
            let cache_key = format!("{job_id}/{virt_key}");
            if state.cache.get(&cache_key).await.is_some() {
                continue;
            }
            let _ = fetch_virtual_with_singleflight(&state, &job_id, &virt_key).await;
        }
    });
}
