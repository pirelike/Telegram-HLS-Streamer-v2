use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};

static WRITE_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

pub fn write_env_values(env_path: &Path, env_map: &HashMap<&str, String>) -> Result<()> {
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
        let Some((key, rest)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let Some(new_value) = env_map.get(key) else {
            continue;
        };
        let inline_comment = find_inline_comment(rest);
        *line = format!("{key}={new_value}{inline_comment}");
        updated.insert(key.to_string());
    }

    for (key, value) in env_map {
        if !updated.contains(*key) {
            lines.push(format!("{key}={value}"));
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
    let tmp_name = tmp_name.replace(
        |c: char| c == ' ' || c == '(' || c == ')' || c == '{' || c == '}',
        "",
    );
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

fn find_inline_comment(rest: &str) -> String {
    for (i, b) in rest.bytes().enumerate() {
        if b == b'#' {
            let before = &rest[..i];
            if !before.ends_with('\\') {
                // A comment must be preceded by whitespace, or be at the start of the string
                if before.chars().last().map_or(true, |c| c.is_whitespace()) {
                    let ws_start = before
                        .rfind(|c: char| !c.is_whitespace())
                        .map(|pos| pos + 1)
                        .unwrap_or(0);
                    return rest[ws_start..].to_string();
                }
            }
        }
    }
    String::new()
}
