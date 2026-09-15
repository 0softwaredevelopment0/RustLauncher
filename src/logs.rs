//! Session logging with numbered files (`logs/launcher-N.log`,
//! `logs/game-N.log`), a port of the Java `FileLogManager`.
//!
//! The counter is monotonic: it scans the existing files and never reuses a
//! number (the Java version kept the counter in SQLite; scanning is simpler
//! and cannot drift).

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::Local;

/// A log file that lines can be pushed into from any thread.
#[allow(dead_code)] // `write` is for callers streaming from multiple threads
pub struct SessionLog {
    inner: Mutex<Option<File>>,
    pub path: PathBuf,
}

impl SessionLog {
    /// Create a new numbered log file in `logs_dir`.
    pub fn start(logs_dir: &Path, prefix: &str) -> Result<SessionLog> {
        std::fs::create_dir_all(logs_dir)
            .with_context(|| format!("failed to create {}", logs_dir.display()))?;
        let num = next_number(logs_dir, prefix);
        let ts = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let path = logs_dir.join(format!("{prefix}-{num}_{ts}.log"));

        let mut file =
            File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
        writeln!(
            file,
            "[Log] === {prefix} session #{num} started at {} ===",
            Local::now().to_rfc3339()
        )?;

        Ok(SessionLog {
            inner: Mutex::new(Some(file)),
            path,
        })
    }

    #[allow(dead_code)]
    pub fn write(&self, line: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{line}");
            }
        }
    }

    /// Close the file. Safe to call twice.
    pub fn finish(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(mut file) = guard.take() {
                let _ = writeln!(
                    file,
                    "[Log] === Log closed at {} ===",
                    Local::now().to_rfc3339()
                );
            }
        }
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        self.finish();
    }
}

/// One past the largest `prefix-N_...` number already on disk.
fn next_number(logs_dir: &Path, prefix: &str) -> u32 {
    let mut max = 0;
    if let Ok(entries) = std::fs::read_dir(logs_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let Some(rest) = name.strip_prefix(prefix) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix('-') else {
                continue;
            };
            if let Some(num_str) = rest.split('_').next() {
                if let Ok(num) = num_str.parse::<u32>() {
                    max = max.max(num);
                }
            }
        }
    }
    max + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rl-log-{}-{tag}", std::process::id()))
    }

    #[test]
    fn numbers_increase_and_lines_are_written() {
        let dir = tmp("counter");
        let _ = std::fs::remove_dir_all(&dir);

        let log1 = SessionLog::start(&dir, "game").unwrap();
        log1.write("line one");
        log1.finish();

        let log2 = SessionLog::start(&dir, "game").unwrap();
        log2.write("line two");
        log2.finish();

        let name1 = log1.path.file_name().unwrap().to_str().unwrap();
        let name2 = log2.path.file_name().unwrap().to_str().unwrap();
        assert!(name1.starts_with("game-1_"), "{name1}");
        assert!(name2.starts_with("game-2_"), "{name2}");
        assert_ne!(name1, name2);

        let content = std::fs::read_to_string(&log2.path).unwrap();
        assert!(content.contains("line two"));
        assert!(content.contains("Log closed"));

        // finish() twice is fine, and finish() keeps the content.
        log2.finish();
        assert_eq!(std::fs::read_to_string(&log2.path).unwrap(), content);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefixes_are_counted_independently() {
        let dir = tmp("prefixes");
        let _ = std::fs::remove_dir_all(&dir);
        let g = SessionLog::start(&dir, "game").unwrap();
        let l = SessionLog::start(&dir, "launcher").unwrap();
        assert!(g
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("game-1_"));
        assert!(l
            .path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("launcher-1_"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
