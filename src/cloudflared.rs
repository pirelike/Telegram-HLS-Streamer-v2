use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::RwLock;

use crate::api::AppState;

#[derive(Debug, Clone, Default, Serialize)]
pub struct CloudflaredStatus {
    pub enabled: bool,
    pub running: bool,
    #[serde(skip_serializing)]
    pub started_at: Option<Instant>,
    pub url: Option<String>,
    pub last_error: Option<String>,
}

impl CloudflaredStatus {
    pub fn uptime_seconds(&self) -> Option<u64> {
        self.started_at.map(|t| t.elapsed().as_secs())
    }
}

pub type SharedCloudflaredStatus = RwLock<CloudflaredStatus>;

pub fn command_args(config_path: &str) -> Vec<String> {
    vec![
        "tunnel".into(),
        "--config".into(),
        config_path.into(),
        "run".into(),
    ]
}

pub fn start_manager(state: Arc<AppState>) {
    tokio::spawn(manager_loop(state));
}

async fn manager_loop(state: Arc<AppState>) {
    loop {
        let cfg = state.config.read().await.clone();
        if !cfg.cloudflared_enabled {
            set_disabled(&state).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        if cfg.cloudflared_config.trim().is_empty() {
            set_error(
                &state,
                true,
                "CLOUDFLARED_CONFIG is required when cloudflared is enabled",
            )
            .await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        let args = command_args(cfg.cloudflared_config.trim());
        let mut cmd = Command::new("cloudflared");
        cmd.args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tracing::info!(args = ?args, "starting cloudflared");
        match cmd.spawn() {
            Ok(mut child) => {
                {
                    let mut status = state.cloudflared.write().await;
                    status.enabled = true;
                    status.running = true;
                    status.started_at = Some(Instant::now());
                    status.last_error = None;
                }
                if let Some(stdout) = child.stdout.take() {
                    tokio::spawn(read_output(state.clone(), stdout));
                }
                if let Some(stderr) = child.stderr.take() {
                    tokio::spawn(read_output(state.clone(), stderr));
                }
                let message = loop {
                    tokio::select! {
                        result = child.wait() => {
                            break match result {
                                Ok(status) => format!("cloudflared exited with {status}"),
                                Err(e) => format!("cloudflared wait failed: {e}"),
                            };
                        }
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {
                            let latest = state.config.read().await.clone();
                            if !latest.cloudflared_enabled {
                                let _ = child.kill().await;
                                set_disabled(&state).await;
                                break "cloudflared disabled".into();
                            }
                            if latest.cloudflared_config.trim() != cfg.cloudflared_config.trim() {
                                let _ = child.kill().await;
                                break "cloudflared config changed".into();
                            }
                        }
                    }
                };
                if message == "cloudflared disabled" {
                    tracing::info!("cloudflared stopped after disable");
                } else {
                    set_error(&state, true, &message).await;
                    tracing::warn!(message, "cloudflared stopped; restarting after delay");
                }
            }
            Err(e) => {
                set_error(&state, true, &format!("failed to start cloudflared: {e}")).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn read_output<R>(state: Arc<AppState>, reader: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(url) = extract_tunnel_url(&line) {
            tracing::info!(%url, "cloudflared tunnel url detected");
            let mut status = state.cloudflared.write().await;
            if status.enabled && status.running {
                status.url = Some(url);
            }
        }
        tracing::info!(target: "cloudflared", %line);
    }
}

fn extract_tunnel_url(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|part| part.starts_with("https://") && part.contains("trycloudflare.com"))
        .map(|part| {
            part.trim_matches(|c: char| c == ',' || c == '.')
                .to_string()
        })
}

async fn set_disabled(state: &AppState) {
    let mut status = state.cloudflared.write().await;
    status.enabled = false;
    status.running = false;
    status.started_at = None;
    status.url = None;
    status.last_error = None;
}

async fn set_error(state: &AppState, enabled: bool, error: &str) {
    let mut status = state.cloudflared.write().await;
    status.enabled = enabled;
    status.running = false;
    status.started_at = None;
    status.url = None;
    status.last_error = Some(error.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_args_use_config_file_mode() {
        assert_eq!(
            command_args("/tmp/config.yml"),
            vec!["tunnel", "--config", "/tmp/config.yml", "run"]
        );
    }

    #[test]
    fn extracts_trycloudflare_url() {
        assert_eq!(
            extract_tunnel_url("INF + https://abc.trycloudflare.com"),
            Some("https://abc.trycloudflare.com".into())
        );
    }
}
