use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::media;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct JobMetadata {
    pub(crate) media_type: Option<String>,
    pub(crate) is_series: Option<bool>,
    pub(crate) series_name: Option<String>,
    pub(crate) season_number: Option<i32>,
    pub(crate) episode_number: Option<i32>,
    pub(crate) part_number: Option<i32>,
    pub(crate) title: Option<String>,
    pub(crate) abr_tiers_override: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub(crate) enum JobStatus {
    Queued,
    Analyzing,
    Processing,
    Uploading,
    Complete,
    Error,
    Cancelled,
}

impl JobStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Analyzing => "analyzing",
            Self::Processing => "processing",
            Self::Uploading => "uploading",
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Error | Self::Cancelled)
    }
}

#[derive(Debug)]
pub(crate) struct JobState {
    pub(crate) job_id: String,
    pub(crate) filename: String,
    pub(crate) source_path: PathBuf,
    pub(crate) processing_path: PathBuf,
    pub(crate) status: JobStatus,
    pub(crate) progress: f64,
    pub(crate) step: u32,
    pub(crate) total_steps: u32,
    pub(crate) description: String,
    pub(crate) queued_at: std::time::Instant,
    pub(crate) started_at: Option<std::time::Instant>,
    pub(crate) finished_at: Option<std::time::Instant>,
    pub(crate) cancel_requested: bool,
    pub(crate) cancel_flag: Arc<AtomicBool>,
    pub(crate) error: Option<String>,
    pub(crate) metadata: JobMetadata,
    pub(crate) analysis: Option<media::MediaAnalysis>,
    pub(crate) delete_source_on_finish: bool,
}

#[derive(Debug)]
pub(crate) struct JobRequest {
    pub(super) job_id: String,
    #[allow(dead_code)]
    pub(super) filename: String,
    pub(super) source_path: PathBuf,
    #[allow(dead_code)]
    pub(super) metadata: JobMetadata,
    pub(super) delete_source_on_finish: bool,
}
