mod download;
mod handlers;
mod json;
pub(crate) mod processing;
mod processing_lifecycle;
mod processing_markers;
mod processing_upload;
mod types;

pub(crate) use processing::{enqueue_existing_job, enqueue_job, start_background_tasks};
pub(crate) use types::JobStatus;
pub(crate) use types::{JobMetadata, JobRequest, JobState};

pub(super) use handlers::{
    handle_active_jobs, handle_cancel_job, handle_delete_job, handle_download_original,
    handle_get_job, handle_job_status, handle_list_jobs, handle_patch_job, handle_reprocess_job,
    queue_metrics,
};

#[cfg(test)]
mod tests;
