use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tauri::Manager;
use url::Url;

pub const MAX_LOG_FILE_BYTES: u128 = 2 * 1024 * 1024;
pub const KEEP_LOG_FILES: usize = 5;

/// Open only while flushing a record. A user can delete the active file or
/// directory at any time; the next record recreates it instead of writing to a
/// deleted file handle. Fern serializes calls to this writer.
struct RecoveringLogWriter {
    dir: PathBuf,
    max_bytes: u64,
    buffer: Vec<u8>,
}

impl RecoveringLogWriter {
    fn new(dir: PathBuf, max_bytes: u64) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("codex-app-manager.log"))?;
        Ok(Self {
            dir,
            max_bytes,
            buffer: Vec::new(),
        })
    }
}

impl Write for RecoveringLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("codex-app-manager.log");
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        if size > 0 && size.saturating_add(self.buffer.len() as u64) > self.max_bytes {
            drop(file);
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let rotated = self.dir.join(format!(
                "codex-app-manager_{stamp}_{}.log",
                uuid::Uuid::new_v4()
            ));
            std::fs::rename(&path, rotated)?;
            file = OpenOptions::new().create(true).append(true).open(&path)?;
            // Never log from inside a logger: its writer lock is already held.
            let _ = prune_log_files(&self.dir, KEEP_LOG_FILES);
        }
        file.write_all(&self.buffer)?;
        file.flush()?;
        self.buffer.clear();
        Ok(())
    }
}

pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let writer = RecoveringLogWriter::new(app.path().app_log_dir()?, MAX_LOG_FILE_BYTES as u64)?;
    app.plugin(
        tauri_plugin_log::Builder::new()
            .targets([
                tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Dispatch(
                    tauri_plugin_log::fern::Dispatch::new().chain(
                        tauri_plugin_log::fern::Output::writer(Box::new(writer), "\n"),
                    ),
                )),
            ])
            .level(if cfg!(debug_assertions) {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            })
            .level_for("tao", log::LevelFilter::Warn)
            .level_for("wry", log::LevelFilter::Warn)
            .format(|out, message, record| {
                out.finish(format_args!(
                    "[{}] [{}] [{}:{}] {}",
                    record.level(),
                    record.target(),
                    record.file().unwrap_or("?"),
                    record.line().unwrap_or(0),
                    message
                ))
            })
            .build(),
    )?;
    Ok(())
}

pub fn logs_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_log_dir().ok()
}

pub fn redact_url(raw: &str) -> String {
    let Ok(url) = Url::parse(raw.trim()) else {
        return "<invalid-url>".to_string();
    };
    let Some(host) = url.host_str() else {
        return "<invalid-url>".to_string();
    };
    let mut redacted = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        redacted.push(':');
        redacted.push_str(&port.to_string());
    }
    redacted
}

pub fn prune_old_logs(dir: &Path, keep: usize) {
    for (path, err) in prune_log_files(dir, keep) {
        log::warn!(
            "failed to prune old log file path={} error={err}",
            path.display()
        );
    }
}

fn prune_log_files(dir: &Path, keep: usize) -> Vec<(PathBuf, io::Error)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // The active log must never be pruned, even if an archive has a newer mtime.
    let archived_keep =
        keep.saturating_sub(usize::from(dir.join("codex-app-manager.log").exists()));
    let mut logs = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name != "codex-app-manager.log"
                        && name.starts_with("codex-app-manager")
                        && name.contains(".log")
                })
        })
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (modified, path)
        })
        .collect::<Vec<_>>();
    logs.sort_by(|(mtime_a, path_a), (mtime_b, path_b)| {
        mtime_b
            .cmp(mtime_a)
            .then_with(|| path_b.file_name().cmp(&path_a.file_name()))
    });
    logs.into_iter()
        .skip(archived_keep)
        .filter_map(|(_, path)| std::fs::remove_file(&path).err().map(|err| (path, err)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::RecoveringLogWriter;
    use super::{prune_old_logs, redact_url};
    use std::io::Write;

    #[test]
    fn deleted_log_file_and_directory_are_recreated_without_restart() {
        let dir = std::env::temp_dir().join(format!("codex-log-recreate-{}", uuid::Uuid::new_v4()));
        let mut writer = RecoveringLogWriter::new(dir.clone(), 1024).unwrap();
        let path = dir.join("codex-app-manager.log");
        writer.write_all(b"before\n").unwrap();
        writer.flush().unwrap();
        std::fs::remove_file(&path).unwrap();
        writer
            .write_all("中文日志 after deletion\n".as_bytes())
            .unwrap();
        writer.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "中文日志 after deletion\n"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        writer.write_all(b"directory recreated\n").unwrap();
        writer.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "directory recreated\n"
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn rotation_bounds_retention_and_always_preserves_active_log() {
        let dir = std::env::temp_dir().join(format!("codex-log-rotation-{}", uuid::Uuid::new_v4()));
        let mut writer = RecoveringLogWriter::new(dir.clone(), 8).unwrap();
        for number in 0..12 {
            writeln!(writer, "record {number}").unwrap();
            writer.flush().unwrap();
        }
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            super::KEEP_LOG_FILES
        );
        let path = dir.join("codex-app-manager.log");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "record 11\n");
        // A newly written archive must not cause pruning of an older active log.
        std::fs::write(dir.join("codex-app-manager_newer.log"), b"archive").unwrap();
        prune_old_logs(&dir, 1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert!(path.exists());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
    }

    #[test]
    fn redact_url_keeps_only_origin() {
        assert_eq!(
            redact_url("https://u:p@example.com:8443/a/b?x=1#frag"),
            "https://example.com:8443"
        );
        assert_eq!(redact_url("http://127.0.0.1/path"), "http://127.0.0.1");
        assert_eq!(redact_url("127.0.0.1/path"), "<invalid-url>");
        assert_eq!(redact_url("not a url"), "<invalid-url>");
    }

    #[test]
    fn prune_old_logs_keeps_newest_by_mtime_then_name() {
        let dir = temp_dir("codex-manager-log-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for idx in 0..7 {
            std::fs::write(dir.join(format!("codex-app-manager.{idx}.log")), b"log").unwrap();
        }
        std::fs::write(dir.join("other.log"), b"keep").unwrap();

        prune_old_logs(&dir, 5);

        let mut names = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                "codex-app-manager.2.log",
                "codex-app-manager.3.log",
                "codex-app-manager.4.log",
                "codex-app-manager.5.log",
                "codex-app-manager.6.log",
                "other.log",
            ]
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_old_logs_tolerates_missing_dir() {
        prune_old_logs(&temp_dir("codex-manager-log-missing"), 5);
    }
}
