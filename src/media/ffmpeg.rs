use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[cfg(unix)]
const SIGTERM_GRACE_SECS: u64 = 10;

pub(super) async fn acquire_ffmpeg_permit(
    semaphore: &Arc<Semaphore>,
    cancel: &Arc<AtomicBool>,
) -> Result<OwnedSemaphorePermit> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        tokio::select! {
            permit = semaphore.clone().acquire_owned() => {
                return permit.context("acquiring ffmpeg encode permit");
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
}

#[cfg(unix)]
pub(super) async fn graceful_kill(child: &mut tokio::process::Child) {
    let pid = match child.id() {
        Some(id) => id as i32,
        None => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return;
        }
    };
    // Send SIGTERM first for graceful shutdown
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    // Wait up to SIGTERM_GRACE_SECS for the process to exit
    match tokio::time::timeout(
        std::time::Duration::from_secs(SIGTERM_GRACE_SECS),
        child.wait(),
    )
    .await
    {
        Ok(Ok(_)) => return, // exited gracefully
        _ => {
            tracing::warn!(pid, "ffmpeg did not exit after SIGTERM; sending SIGKILL");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

#[cfg(not(unix))]
pub(super) async fn graceful_kill(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

pub(super) async fn run_ffmpeg_cancellable(
    cmd: &mut Command,
    cancel: &Arc<AtomicBool>,
    timeout_secs: u64,
) -> Result<()> {
    tracing::debug!(cmd = ?cmd, "ffmpeg spawn");
    let started = Instant::now();
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("spawning ffmpeg")?;

    // Drain stderr in background so FFmpeg never blocks on a full pipe.
    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut buf = Vec::with_capacity(8192);
            let mut tmp = [0u8; 4096];
            loop {
                match stderr.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.len() > 8192 {
                            let keep = 8192;
                            buf = buf.split_off(buf.len() - keep);
                        }
                    }
                    Err(_) => break,
                }
            }
            buf
        })
    });

    let timeout_secs = timeout_secs.max(1);
    let timeout = tokio::time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(timeout);

    let exit_status = loop {
        tokio::select! {
            status = child.wait() => break status,
            _ = &mut timeout => {
                tracing::warn!("ffmpeg per-process timeout reached; sending SIGTERM");
                graceful_kill(&mut child).await;
                if let Some(h) = stderr_task { h.abort(); }
                bail!("ffmpeg timed out after {} seconds", timeout_secs);
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if cancel.load(Ordering::Relaxed) {
                    tracing::warn!("ffmpeg cancelled; sending SIGTERM");
                    graceful_kill(&mut child).await;
                    if let Some(h) = stderr_task { h.abort(); }
                    bail!("cancelled");
                }
            }
        }
    };

    let stderr_bytes = if let Some(h) = stderr_task {
        h.await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let exit_status = exit_status.context("waiting for ffmpeg")?;
    if exit_status.success() {
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "ffmpeg complete"
        );
        return Ok(());
    }
    tracing::warn!(
        elapsed_ms = started.elapsed().as_millis(),
        stderr = %String::from_utf8_lossy(&stderr_bytes).trim(),
        "ffmpeg failed"
    );
    bail!(
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&stderr_bytes).trim()
    )
}
