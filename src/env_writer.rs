use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};

static WRITE_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn contains_control_chars(value: &str) -> bool {
    value.chars().any(|c| c == '\n' || c == '\r' || c == '\0')
}

pub fn write_env_values(env_path: &Path, env_map: &HashMap<&str, String>) -> Result<()> {
    for (key, value) in env_map {
        if contains_control_chars(value) {
            bail!("setting {} contains control characters", key);
        }
    }

    let mutex = WRITE_MUTEX.get_or_init(|| Mutex::new(()));
    let _guard = mutex.lock().unwrap_or_else(|e| e.into_inner());

    let existing = std::fs::read_to_string(env_path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();
    let mut updated: HashSet<String> = HashSet::new();

    for line in lines.iter_mut() {
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, _)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let Some(new_value) = env_map.get(key) else {
            continue;
        };
        *line = format!("{key}={}", encode_env_value(new_value));
        updated.insert(key.to_string());
    }

    for (key, value) in env_map {
        if !updated.contains(*key) {
            lines.push(format!("{key}={}", encode_env_value(value)));
        }
    }

    let pid = std::process::id();
    let thread_id = std::thread::current().id();
    let tmp_name = format!(
        "{}.tmp-{:?}-{:?}",
        env_path.file_name().unwrap_or_default().to_string_lossy(),
        pid,
        thread_id
    );
    // Remove characters that might be invalid in paths (like spaces or special punctuation in ThreadId format)
    let tmp_name = tmp_name.replace([' ', '(', ')', '{', '}'], "");
    let tmp = env_path.with_file_name(tmp_name);

    let content = lines.join("\n");
    let content = if content.ends_with('\n') {
        content
    } else {
        format!("{content}\n")
    };
    let mut f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(content.as_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync {}", tmp.display()))?;
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, env_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), env_path.display()));
    }

    Ok(())
}

fn encode_env_value(value: &str) -> String {
    if value.contains('#') || value.contains('"') || value.contains('\\') {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn rejects_newline_in_value() {
        let mut map = HashMap::new();
        map.insert("TEST_KEY", "value\nINJECTED=key".to_string());
        let result = write_env_values(std::path::Path::new("/dev/null"), &map);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("control characters"), "error: {err}");
    }

    #[test]
    fn rejects_carriage_return_in_value() {
        let mut map = HashMap::new();
        map.insert("TEST_KEY", "value\rINJECTED".to_string());
        let result = write_env_values(std::path::Path::new("/dev/null"), &map);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_null_byte_in_value() {
        let mut map = HashMap::new();
        map.insert("TEST_KEY", "value\0injected".to_string());
        let result = write_env_values(std::path::Path::new("/dev/null"), &map);
        assert!(result.is_err());
    }

    #[test]
    fn accepts_clean_value() {
        let mut map = HashMap::new();
        map.insert("TEST_KEY", "normal_value".to_string());
        // Use a temp dir to avoid writing to real .env
        let dir = std::env::temp_dir().join(format!("thls_env_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(".env.test");
        let result = write_env_values(&path, &map);
        assert!(result.is_ok(), "should accept clean value: {:?}", result);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn quotes_hash_values_and_drops_stale_inline_comment() {
        let mut map = HashMap::new();
        map.insert("WEBHOOK_URL", "https://example.test/hook#frag".to_string());
        let dir = std::env::temp_dir().join(format!("thls_env_hash_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(".env.test");
        std::fs::write(&path, "WEBHOOK_URL=https://old.test # old comment\n").unwrap();

        write_env_values(&path, &map).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "WEBHOOK_URL=\"https://example.test/hook#frag\"\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
