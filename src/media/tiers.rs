use crate::config::Config;

use super::models::VideoTier;

pub fn select_video_tiers(cfg: &Config, codec: &str, source_height: i64) -> Vec<VideoTier> {
    let copyable = matches!(codec, "h264" | "hevc");
    let effective_abr = cfg.abr_enabled && !cfg.virtual_abr_tiers;
    let copy_tier0 = cfg.enable_copy_mode && copyable;
    let tier0_bitrate = if copy_tier0 {
        "copy".to_string()
    } else {
        tier0_bitrate(cfg, source_height)
    };
    let mut tiers = vec![VideoTier {
        index: 0,
        height: source_height,
        bitrate: tier0_bitrate,
        copy: copy_tier0,
    }];
    if effective_abr {
        for (height, bitrate) in parse_tiers(&cfg.abr_tiers) {
            let include = if copy_tier0 {
                height < source_height
            } else {
                height <= source_height
            };
            if include {
                tiers.push(VideoTier {
                    index: tiers.len(),
                    height,
                    bitrate,
                    copy: false,
                });
            }
        }
    }
    tiers
}

pub fn select_video_tiers_with(
    cfg: &Config,
    codec: &str,
    source_height: i64,
    tiers_raw: &str,
) -> Vec<VideoTier> {
    let copyable = matches!(codec, "h264" | "hevc");
    let copy_tier0 = cfg.enable_copy_mode && copyable;
    let tier0_bitrate = if copy_tier0 {
        "copy".to_string()
    } else {
        tier0_bitrate(cfg, source_height)
    };
    let mut tiers = vec![VideoTier {
        index: 0,
        height: source_height,
        bitrate: tier0_bitrate,
        copy: copy_tier0,
    }];
    // Virtual ABR disables eager tiers; honor that even with an override.
    if cfg.abr_enabled && !cfg.virtual_abr_tiers {
        for (height, bitrate) in parse_tiers(tiers_raw) {
            let include = if copy_tier0 {
                height < source_height
            } else {
                height <= source_height
            };
            if include {
                tiers.push(VideoTier {
                    index: tiers.len(),
                    height,
                    bitrate,
                    copy: false,
                });
            }
        }
    }
    tiers
}

pub fn parse_tiers(raw: &str) -> Vec<(i64, String)> {
    let mut tiers = parse_tiers_in_order(raw);
    tiers.sort_by_key(|b| std::cmp::Reverse(b.0));
    tiers
}

pub fn parse_tiers_in_order(raw: &str) -> Vec<(i64, String)> {
    raw.split(',')
        .filter_map(|pair| {
            let (height, bitrate) = pair.split_once(':')?;
            let height = height.trim().parse::<i64>().ok()?;
            Some((height, bitrate.trim().to_string()))
        })
        .collect::<Vec<_>>()
}

pub fn tier_bitrate(raw: &str, target_height: i64) -> Option<String> {
    parse_tiers_in_order(raw)
        .into_iter()
        .find(|(height, _)| *height == target_height)
        .map(|(_, bitrate)| bitrate)
}

pub(super) fn tier0_bitrate(cfg: &Config, source_height: i64) -> String {
    let mut selected: Option<(i64, String)> = None;
    for (height, bitrate) in parse_tiers(&cfg.tier0_bitrates) {
        if height <= source_height && selected.as_ref().map(|s| height > s.0).unwrap_or(true) {
            selected = Some((height, bitrate));
        }
    }
    selected
        .map(|(_, bitrate)| bitrate)
        .unwrap_or_else(|| cfg.tier0_bitrate_default.clone())
}
