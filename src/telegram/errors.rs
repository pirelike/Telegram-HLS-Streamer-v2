use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use tokio::time::Duration;

pub(super) fn api_url(base_url: &str, token: &str, method: &str) -> String {
    format!("{}/bot{token}/{method}", base_url.trim_end_matches('/'))
}

pub(super) fn file_url(base_url: &str, token: &str, file_path: &str) -> String {
    format!(
        "{}/file/bot{token}/{}",
        base_url.trim_end_matches('/'),
        file_path.trim_start_matches('/')
    )
}

pub(super) fn validate_file_id(file_id: &str) -> Result<()> {
    if file_id.len() < 5 {
        tracing::warn!(
            len = file_id.len(),
            "validate_file_id called with suspiciously short file_id"
        );
    }
    if (50..=255).contains(&file_id.len())
        && file_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        bail!("invalid Telegram file_id (expected 50–255 chars of A-Za-z0-9/_-)")
    }
}

pub(super) fn redact_bot_token(msg: &str) -> String {
    let mut result = msg.to_string();
    let mut pos = 0;
    while let Some(offset) = result[pos..].find("/bot") {
        let start = pos + offset;
        let token_start = start + 4;
        let Some(end_offset) = result[token_start..].find('/') else {
            break;
        };
        let end = token_start + end_offset;
        if end - token_start <= 10 {
            break;
        }
        result.replace_range(token_start..end, "***REDACTED***");
        pos = token_start + "***REDACTED***".len();
    }
    result
}

pub(super) fn classify_reqwest(e: reqwest::Error) -> TelegramError {
    let sanitized = redact_bot_token(&e.to_string());
    if e.is_timeout() || e.is_connect() || e.is_request() {
        TelegramError::Retryable(anyhow!("{}", sanitized))
    } else {
        TelegramError::Permanent(anyhow!("{}", sanitized))
    }
}

pub(super) fn classify_api_body(status: u16, body: &Value) -> TelegramError {
    let code = body
        .get("error_code")
        .and_then(Value::as_u64)
        .unwrap_or(status as u64);
    if code == 429 {
        let retry_after = body
            .pointer("/parameters/retry_after")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        return TelegramError::RetryAfter(Duration::from_secs(retry_after));
    }
    if code == 400 || code == 403 {
        return TelegramError::Permanent(anyhow!(normalize_telegram_api_error(status, body)));
    }
    TelegramError::Retryable(anyhow!(normalize_telegram_api_error(status, body)))
}

pub(super) fn normalize_telegram_api_error(status: u16, body: &Value) -> String {
    if status == 403 {
        return "forbidden".into();
    }
    if status == 429 {
        if let Some(retry_after) = body
            .pointer("/parameters/retry_after")
            .and_then(Value::as_i64)
        {
            return format!("rate_limited:{retry_after}");
        }
        return "rate_limited".into();
    }
    let description = body
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("telegram api error");
    format!("telegram_api: {}", truncate(description, 120))
}

pub(super) fn normalize_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() || e.is_request() {
        "network_error".into()
    } else {
        format!(
            "reqwest: {}",
            truncate(&redact_bot_token(&e.to_string()), 120)
        )
    }
}

pub(super) fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        value.to_string()
    } else {
        let mut boundary = limit;
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value[..boundary].to_string()
    }
}

#[derive(Debug)]
pub(super) enum TelegramError {
    Retryable(anyhow::Error),
    RetryAfter(Duration),
    Permanent(anyhow::Error),
}

impl TelegramError {
    pub(super) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Retryable(e) | Self::Permanent(e) => e,
            Self::RetryAfter(wait) => anyhow!("telegram rate limited after retries: {wait:?}"),
        }
    }
}
