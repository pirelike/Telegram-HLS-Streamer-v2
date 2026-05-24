use std::process::Stdio;

use tokio::process::Command;

use super::models::SelectedEncoder;
use crate::config::Config;

pub async fn select_encoder(cfg: &Config) -> SelectedEncoder {
    if !cfg.enable_hw_accel || cfg.preferred_encoder == "cpu" {
        return cpu_encoder();
    }

    if cfg.preferred_encoder == "vaapi" {
        let device = vaapi_device(cfg);
        let encoder = SelectedEncoder {
            name: "h264_vaapi".to_string(),
            vaapi_device: device,
        };
        if encoder_probe_ok(&encoder).await {
            return encoder;
        }
        return cpu_encoder();
    }

    let name = match cfg.preferred_encoder.as_str() {
        "nvenc" => "h264_nvenc",
        "qsv" => "h264_qsv",
        _ => "libx264",
    };
    let encoder = SelectedEncoder {
        name: name.to_string(),
        vaapi_device: None,
    };
    if encoder_probe_ok(&encoder).await {
        encoder
    } else {
        cpu_encoder()
    }
}

pub fn cpu_encoder() -> SelectedEncoder {
    SelectedEncoder {
        name: "libx264".to_string(),
        vaapi_device: None,
    }
}

fn vaapi_device(cfg: &Config) -> Option<String> {
    if !cfg.vaapi_device.is_empty() {
        return Some(cfg.vaapi_device.clone());
    }
    let entries = std::fs::read_dir("/dev/dri").ok()?;
    let mut devices = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let n = name.strip_prefix("renderD")?.parse::<u32>().ok()?;
            Some((n, entry.path().to_string_lossy().to_string()))
        })
        .collect::<Vec<_>>();
    devices.sort_by_key(|(n, _)| *n);
    devices.first().map(|(_, path)| path.clone())
}

async fn encoder_probe_ok(encoder: &SelectedEncoder) -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner").arg("-v").arg("error");
    add_encoder_device_args(&mut cmd, encoder);
    cmd.arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("color=black:s=128x128:d=0.1");
    if let Some(filter) = video_filter(encoder, None) {
        cmd.arg("-vf").arg(filter);
    }
    cmd.arg("-c:v")
        .arg(&encoder.name)
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.status()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "encoder probe command failed");
            false
        }
        Err(_) => {
            tracing::warn!("encoder probe timed out after 30s");
            false
        }
    }
}

pub fn video_filter(encoder: &SelectedEncoder, scale: Option<String>) -> Option<String> {
    if encoder.name.ends_with("_vaapi") {
        Some(match scale {
            Some(scale) => format!("{scale},format=nv12,hwupload"),
            None => "format=nv12,hwupload".to_string(),
        })
    } else {
        scale
    }
}

pub fn add_forced_idr_args(cmd: &mut Command, encoder: &SelectedEncoder) {
    if encoder.name == "h264_qsv" {
        cmd.arg("-forced_idr").arg("1");
    } else if encoder.name == "libx264" || encoder.name == "h264_nvenc" {
        cmd.arg("-forced-idr").arg("1");
    }
}

pub fn add_encoder_device_args(cmd: &mut Command, encoder: &SelectedEncoder) {
    if let Some(device) = &encoder.vaapi_device {
        cmd.arg("-vaapi_device").arg(device);
    }
}
