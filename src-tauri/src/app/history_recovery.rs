use serde::Serialize;
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const RECOVERY_ORIGINATOR: &str = "Codex history evidence recovery";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingRollout {
    Native,
    IdMismatch,
    GeneratedLegacy,
    GeneratedDesktopReadable,
}

pub(crate) struct PreparedRecoveryUpgrade {
    pub original: Vec<u8>,
    pub upgraded: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeneratedRecoveryUpgradePlan {
    NotNeeded,
    Planned(u64),
    ExceedsOutputLimit,
}

struct RecoveryMetadata {
    created_at: i64,
    cwd: String,
    model_provider: String,
    cli_version: String,
}

struct BoundedWriter<W> {
    inner: W,
    written: u64,
    max_bytes: u64,
    overflowed: bool,
}

impl<W> BoundedWriter<W> {
    fn new(inner: W, max_bytes: u64) -> Self {
        Self {
            inner,
            written: 0,
            max_bytes,
            overflowed: false,
        }
    }

    fn into_inner(self) -> W {
        self.inner
    }

    fn written(&self) -> u64 {
        self.written
    }
}

impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.max_bytes.saturating_sub(self.written) {
            self.overflowed = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "recovery rollout exceeds output limit",
            ));
        }
        let written = self.inner.write(bytes)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Serialize)]
struct DesktopRecord<P> {
    timestamp: String,
    #[serde(rename = "type")]
    record_type: &'static str,
    payload: P,
}

#[derive(Serialize)]
struct SessionMetaPayload<'a> {
    id: &'a str,
    timestamp: &'a str,
    cwd: &'a str,
    originator: &'static str,
    cli_version: &'a str,
    source: &'static str,
    model_provider: &'a str,
    thread_source: &'static str,
}

#[derive(Serialize)]
struct TaskStartedPayload<'a> {
    #[serde(rename = "type")]
    payload_type: &'static str,
    turn_id: &'a str,
    started_at: i64,
    model_context_window: Option<u64>,
    collaboration_mode_kind: &'static str,
}

#[derive(Serialize)]
struct UserMessagePayload<'a, 'media> {
    #[serde(rename = "type")]
    payload_type: &'static str,
    message: &'a str,
    images: &'a [&'media str],
    local_images: &'static [()],
    audio: &'a [&'media str],
    local_audio: &'static [()],
    text_elements: &'static [()],
}

struct VisibleUserMessage<'a> {
    message: String,
    images: Vec<&'a str>,
    audio: Vec<&'a str>,
}

#[derive(Serialize)]
struct AgentMessagePayload<'a> {
    #[serde(rename = "type")]
    payload_type: &'static str,
    message: &'a str,
    phase: Option<()>,
    memory_citation: Option<()>,
}

#[derive(Serialize)]
struct TaskCompletePayload<'a> {
    #[serde(rename = "type")]
    payload_type: &'static str,
    turn_id: &'a str,
    last_agent_message: Option<&'a str>,
    started_at: i64,
    completed_at: i64,
    duration_ms: i64,
}

fn parse_recovery_metadata(
    text: &str,
    expected_id: &str,
) -> Result<
    (ExistingRollout, Option<RecoveryMetadata>),
    crate::app::conversation_alignment::AlignmentError,
> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(first_line) = lines.next() else {
        return Ok((ExistingRollout::Native, None));
    };
    let meta = serde_json::from_str::<Value>(first_line)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let mut has_task_started = false;
    let mut has_user_message = false;
    for line in lines {
        let record = serde_json::from_str::<Value>(line)
            .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
        if record.get("type").and_then(Value::as_str) == Some("event_msg") {
            match record.pointer("/payload/type").and_then(Value::as_str) {
                Some("task_started") => has_task_started = true,
                Some("user_message") => has_user_message = true,
                _ => {}
            }
        }
    }

    let class = if meta.get("type").and_then(Value::as_str) != Some("session_meta")
        || meta.pointer("/payload/id").and_then(Value::as_str) != Some(expected_id)
    {
        ExistingRollout::IdMismatch
    } else if meta.pointer("/payload/originator").and_then(Value::as_str)
        != Some(RECOVERY_ORIGINATOR)
    {
        ExistingRollout::Native
    } else if has_task_started && has_user_message {
        ExistingRollout::GeneratedDesktopReadable
    } else {
        ExistingRollout::GeneratedLegacy
    };
    if class != ExistingRollout::GeneratedLegacy {
        return Ok((class, None));
    }

    let timestamp = meta
        .pointer("/payload/timestamp")
        .and_then(Value::as_str)
        .or_else(|| meta.get("timestamp").and_then(Value::as_str))
        .ok_or(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let created_at = OffsetDateTime::parse(timestamp, &Rfc3339)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?
        .unix_timestamp();
    Ok((
        class,
        Some(RecoveryMetadata {
            created_at,
            cwd: meta
                .pointer("/payload/cwd")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            model_provider: meta
                .pointer("/payload/model_provider")
                .and_then(Value::as_str)
                .unwrap_or("custom")
                .to_string(),
            cli_version: meta
                .pointer("/payload/cli_version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
    ))
}

fn recovery_text_without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn write_record<W: Write, P: Serialize>(
    output: &mut BoundedWriter<W>,
    record: &DesktopRecord<P>,
) -> Result<(), crate::app::conversation_alignment::AlignmentError> {
    if serde_json::to_writer(&mut *output, record).is_err() {
        return Err(if output.overflowed {
            crate::app::conversation_alignment::AlignmentError::BlockedMetadata
        } else {
            crate::app::conversation_alignment::AlignmentError::Mutation
        });
    }
    output.write_all(b"\n").map_err(|_| {
        if output.overflowed {
            crate::app::conversation_alignment::AlignmentError::BlockedMetadata
        } else {
            crate::app::conversation_alignment::AlignmentError::Mutation
        }
    })
}

fn shifted_time(
    created_at: i64,
    offset: i64,
) -> Result<i64, crate::app::conversation_alignment::AlignmentError> {
    created_at
        .checked_add(offset)
        .ok_or(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)
}

fn write_task_complete<W: Write>(
    output: &mut BoundedWriter<W>,
    turn: &mut Option<(String, i64, Option<String>)>,
    created_at: i64,
    offset: &mut i64,
) -> Result<(), crate::app::conversation_alignment::AlignmentError> {
    if let Some((turn_id, started_at, last_agent_message)) = turn.take() {
        let completed_at = shifted_time(created_at, *offset)?;
        let timestamp = recovery_timestamp(completed_at)?;
        write_record(
            output,
            &DesktopRecord {
                timestamp,
                record_type: "event_msg",
                payload: TaskCompletePayload {
                    payload_type: "task_complete",
                    turn_id: &turn_id,
                    last_agent_message: last_agent_message.as_deref(),
                    started_at,
                    completed_at,
                    duration_ms: completed_at.saturating_sub(started_at).saturating_mul(1000),
                },
            },
        )?;
        *offset += 1;
    }
    Ok(())
}

fn write_desktop_recovery<W: Write>(
    output: &mut BoundedWriter<W>,
    thread_id: &str,
    metadata: &RecoveryMetadata,
    model_provider: &str,
    source_text: &str,
) -> Result<(), crate::app::conversation_alignment::AlignmentError> {
    uuid::Uuid::parse_str(thread_id)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let timestamp = recovery_timestamp(metadata.created_at)?;
    write_record(
        output,
        &DesktopRecord {
            timestamp: timestamp.clone(),
            record_type: "session_meta",
            payload: SessionMetaPayload {
                id: thread_id,
                timestamp: &timestamp,
                cwd: &metadata.cwd,
                originator: RECOVERY_ORIGINATOR,
                cli_version: &metadata.cli_version,
                source: "cli",
                model_provider,
                thread_source: "user",
            },
        },
    )?;

    let mut offset = 1_i64;
    let mut turn: Option<(String, i64, Option<String>)> = None;
    let mut has_user_message = false;
    for line in source_text.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<Value>(line)
            .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
        if record.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let message = record
            .get("payload")
            .ok_or(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
        let user_message = visible_user_message(message);
        let assistant_text = visible_assistant_text(message);
        if let Some(user_message) = user_message {
            write_task_complete(output, &mut turn, metadata.created_at, &mut offset)?;
            let turn_id = uuid::Uuid::new_v4().to_string();
            let started_at = shifted_time(metadata.created_at, offset)?;
            write_record(
                output,
                &DesktopRecord {
                    timestamp: recovery_timestamp(started_at)?,
                    record_type: "event_msg",
                    payload: TaskStartedPayload {
                        payload_type: "task_started",
                        turn_id: &turn_id,
                        started_at,
                        model_context_window: None,
                        collaboration_mode_kind: "default",
                    },
                },
            )?;
            offset += 1;
            write_record(
                output,
                &DesktopRecord {
                    timestamp: recovery_timestamp(shifted_time(metadata.created_at, offset)?)?,
                    record_type: "event_msg",
                    payload: UserMessagePayload {
                        payload_type: "user_message",
                        message: &user_message.message,
                        images: &user_message.images,
                        local_images: &[],
                        audio: &user_message.audio,
                        local_audio: &[],
                        text_elements: &[],
                    },
                },
            )?;
            offset += 1;
            turn = Some((turn_id, started_at, None));
            has_user_message = true;
        }

        write_record(
            output,
            &DesktopRecord {
                timestamp: recovery_timestamp(shifted_time(metadata.created_at, offset)?)?,
                record_type: "response_item",
                payload: message,
            },
        )?;
        offset += 1;
        if let Some(text) = assistant_text {
            write_record(
                output,
                &DesktopRecord {
                    timestamp: recovery_timestamp(shifted_time(metadata.created_at, offset)?)?,
                    record_type: "event_msg",
                    payload: AgentMessagePayload {
                        payload_type: "agent_message",
                        message: &text,
                        phase: None,
                        memory_citation: None,
                    },
                },
            )?;
            offset += 1;
            if let Some((_, _, last_agent_message)) = turn.as_mut() {
                *last_agent_message = Some(text);
            }
        }
    }
    write_task_complete(output, &mut turn, metadata.created_at, &mut offset)?;
    if !has_user_message {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    Ok(())
}

fn generated_rollout_needs_desktop_upgrade_internal(
    path: &Path,
    expected_id: &str,
    target_provider: Option<&str>,
    input_limit: u64,
    output_limit: u64,
) -> Result<GeneratedRecoveryUpgradePlan, crate::app::conversation_alignment::AlignmentError> {
    let bytes = read_recovery_bytes(path, input_limit)?;
    let input_bytes = bytes.len() as u64;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let text = recovery_text_without_bom(text);
    let (class, metadata) = parse_recovery_metadata(text, expected_id)?;
    if class != ExistingRollout::GeneratedLegacy {
        return Ok(GeneratedRecoveryUpgradePlan::NotNeeded);
    }
    let metadata =
        metadata.ok_or(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let provider = target_provider.unwrap_or(&metadata.model_provider);
    let mut output = BoundedWriter::new(std::io::sink(), output_limit);
    if let Err(error) = write_desktop_recovery(&mut output, expected_id, &metadata, provider, text)
    {
        if output.overflowed {
            return Ok(GeneratedRecoveryUpgradePlan::ExceedsOutputLimit);
        }
        return Err(error);
    }
    input_bytes
        .checked_add(output.written())
        .map(GeneratedRecoveryUpgradePlan::Planned)
        .ok_or(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)
}

pub(crate) fn planned_generated_rollout_upgrade_with_limits(
    path: &Path,
    expected_id: &str,
    target_provider: &str,
    input_limit: u64,
    output_limit: u64,
) -> Result<GeneratedRecoveryUpgradePlan, crate::app::conversation_alignment::AlignmentError> {
    generated_rollout_needs_desktop_upgrade_internal(
        path,
        expected_id,
        Some(target_provider),
        input_limit,
        output_limit,
    )
}

#[cfg(test)]
fn generated_rollout_needs_desktop_upgrade_with_limits(
    path: &Path,
    expected_id: &str,
    input_limit: u64,
    output_limit: u64,
) -> Result<bool, crate::app::conversation_alignment::AlignmentError> {
    match generated_rollout_needs_desktop_upgrade_internal(
        path,
        expected_id,
        None,
        input_limit,
        output_limit,
    )? {
        GeneratedRecoveryUpgradePlan::NotNeeded => Ok(false),
        GeneratedRecoveryUpgradePlan::Planned(_) => Ok(true),
        GeneratedRecoveryUpgradePlan::ExceedsOutputLimit => {
            Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)
        }
    }
}

#[cfg(test)]
fn classify_existing_rollout(expected_id: &str, records: &[Value]) -> ExistingRollout {
    let Some(meta) = records.first() else {
        return ExistingRollout::Native;
    };
    if meta.get("type").and_then(Value::as_str) != Some("session_meta")
        || meta.pointer("/payload/id").and_then(Value::as_str) != Some(expected_id)
    {
        return ExistingRollout::IdMismatch;
    }
    if meta.pointer("/payload/originator").and_then(Value::as_str) != Some(RECOVERY_ORIGINATOR) {
        return ExistingRollout::Native;
    }

    let has_task_started = records.iter().any(|record| {
        record.get("type").and_then(Value::as_str) == Some("event_msg")
            && record.pointer("/payload/type").and_then(Value::as_str) == Some("task_started")
    });
    let has_user_message = records.iter().any(|record| {
        record.get("type").and_then(Value::as_str) == Some("event_msg")
            && record.pointer("/payload/type").and_then(Value::as_str) == Some("user_message")
    });
    if has_task_started && has_user_message {
        ExistingRollout::GeneratedDesktopReadable
    } else {
        ExistingRollout::GeneratedLegacy
    }
}

fn recovery_timestamp(
    unix_seconds: i64,
) -> Result<String, crate::app::conversation_alignment::AlignmentError> {
    OffsetDateTime::from_unix_timestamp(unix_seconds)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?
        .format(&Rfc3339)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)
}

fn visible_user_message(message: &Value) -> Option<VisibleUserMessage<'_>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let content = message.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let images = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("input_image"))
        .filter_map(|part| part.get("image_url").and_then(Value::as_str))
        .filter(|url| !url.is_empty())
        .collect::<Vec<_>>();
    let audio = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("input_audio"))
        .filter_map(|part| part.get("audio_url").and_then(Value::as_str))
        .filter(|url| !url.is_empty())
        .collect::<Vec<_>>();
    (!text.is_empty() || !images.is_empty() || !audio.is_empty()).then_some(VisibleUserMessage {
        message: text,
        images,
        audio,
    })
}

fn visible_assistant_text(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let text = message
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
fn desktop_recovery_lines(
    thread_id: &str,
    created_at: i64,
    cwd: &str,
    model_provider: &str,
    cli_version: &str,
    messages: &[Value],
) -> Result<Vec<String>, crate::app::conversation_alignment::AlignmentError> {
    let source_text = messages
        .iter()
        .map(|message| {
            json!({
                "timestamp": recovery_timestamp(created_at).unwrap(),
                "type": "response_item",
                "payload": message
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let metadata = RecoveryMetadata {
        created_at,
        cwd: cwd.to_string(),
        model_provider: model_provider.to_string(),
        cli_version: cli_version.to_string(),
    };
    let mut output = BoundedWriter::new(
        Vec::new(),
        crate::app::conversation_alignment::MAX_RECOVERY_ROLLOUT_BYTES,
    );
    write_desktop_recovery(
        &mut output,
        thread_id,
        &metadata,
        model_provider,
        &source_text,
    )?;
    let bytes = output.into_inner();
    String::from_utf8(bytes)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::Mutation)
        .map(|text| text.lines().map(str::to_string).collect())
}

pub(crate) fn prepare_generated_recovery_upgrade(
    path: &Path,
    expected_id: &str,
    target_provider: Option<&str>,
) -> Result<Option<PreparedRecoveryUpgrade>, crate::app::conversation_alignment::AlignmentError> {
    let original = read_recovery_bytes(
        path,
        crate::app::conversation_alignment::MAX_RECOVERY_ROLLOUT_BYTES,
    )?;
    let text = std::str::from_utf8(&original)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let text = recovery_text_without_bom(text);
    let (class, metadata) = parse_recovery_metadata(text, expected_id)?;
    if class != ExistingRollout::GeneratedLegacy {
        return Ok(None);
    }
    let metadata =
        metadata.ok_or(crate::app::conversation_alignment::AlignmentError::BlockedMetadata)?;
    let provider = target_provider.unwrap_or(&metadata.model_provider);
    let mut output = BoundedWriter::new(
        Vec::new(),
        crate::app::conversation_alignment::MAX_RECOVERY_ROLLOUT_BYTES,
    );
    write_desktop_recovery(&mut output, expected_id, &metadata, provider, text)?;
    let upgraded = output.into_inner();
    Ok(Some(PreparedRecoveryUpgrade { original, upgraded }))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn map_recovery_io(
    error: std::io::Error,
) -> crate::app::conversation_alignment::AlignmentError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        crate::app::conversation_alignment::AlignmentError::PermissionDenied
    } else {
        crate::app::conversation_alignment::AlignmentError::Mutation
    }
}

pub(crate) fn read_recovery_bytes(
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, crate::app::conversation_alignment::AlignmentError> {
    let metadata = fs::symlink_metadata(path).map_err(map_recovery_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > max_bytes {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(map_recovery_io)?
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(map_recovery_io)?;
    if bytes.len() as u64 > max_bytes {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    Ok(bytes)
}

#[cfg(unix)]
fn preserve_source_permissions(path: &Path, permissions: fs::Permissions) -> std::io::Result<()> {
    fs::set_permissions(path, permissions)?;
    fs::File::open(path)?.sync_all()
}

#[cfg(target_os = "windows")]
fn open_replacement_guard(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(path)
}

#[cfg(not(target_os = "windows"))]
fn open_replacement_guard(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

fn read_bounded_file(
    source: &mut fs::File,
    max_bytes: u64,
) -> Result<Vec<u8>, crate::app::conversation_alignment::AlignmentError> {
    let metadata = source.metadata().map_err(map_recovery_io)?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    source.seek(SeekFrom::Start(0)).map_err(map_recovery_io)?;
    let mut bytes = Vec::new();
    source
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(map_recovery_io)?;
    if bytes.len() as u64 > max_bytes {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn preserve_source_permissions(_path: &Path, _permissions: fs::Permissions) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(not(unix), not(target_os = "windows")))]
fn preserve_source_permissions(path: &Path, permissions: fs::Permissions) -> std::io::Result<()> {
    fs::set_permissions(path, permissions)
}

pub(crate) fn replace_recovery_rollout(
    path: &Path,
    expected_sha256: &str,
    replacement: &[u8],
    keep_sidecar_backup: bool,
    replacement_landed: &mut bool,
    idle_check: &dyn Fn() -> Result<(), crate::app::conversation_alignment::AlignmentError>,
) -> Result<Option<PathBuf>, crate::app::conversation_alignment::AlignmentError> {
    *replacement_landed = false;
    let original = read_recovery_bytes(
        path,
        crate::app::conversation_alignment::MAX_RECOVERY_ROLLOUT_BYTES,
    )?;
    if sha256_hex(&original) != expected_sha256 {
        return Err(crate::app::conversation_alignment::AlignmentError::StaleRecovery);
    }
    let parent = path
        .parent()
        .ok_or(crate::app::conversation_alignment::AlignmentError::UnsafePath)?;
    let source_permissions = fs::metadata(path).map_err(map_recovery_io)?.permissions();

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| crate::app::conversation_alignment::AlignmentError::Mutation)?
        .as_millis();
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(crate::app::conversation_alignment::AlignmentError::UnsafePath)?;
    let backup = path.with_file_name(format!("{filename}.pre-desktop-turns-{stamp}.bak"));
    let temporary = path.with_file_name(format!(".{filename}.desktop-turns-{stamp}.tmp"));
    if keep_sidecar_backup {
        crate::app::conversation_alignment::write_private_new(&backup, &original)
            .map_err(map_recovery_io)?;
        crate::app::conversation_alignment::sync_dir(parent).map_err(map_recovery_io)?;
    }
    if let Err(error) =
        crate::app::conversation_alignment::write_private_new(&temporary, replacement)
    {
        if keep_sidecar_backup {
            let _ = fs::remove_file(&backup);
        }
        return Err(map_recovery_io(error));
    }
    if let Err(error) = preserve_source_permissions(&temporary, source_permissions) {
        let _ = fs::remove_file(&temporary);
        if keep_sidecar_backup {
            let _ = fs::remove_file(&backup);
        }
        return Err(map_recovery_io(error));
    }
    let guarded_replace = (|| {
        let mut source_guard = open_replacement_guard(path).map_err(map_recovery_io)?;
        idle_check()?;
        let current = read_bounded_file(
            &mut source_guard,
            crate::app::conversation_alignment::MAX_RECOVERY_ROLLOUT_BYTES,
        )?;
        if sha256_hex(&current) != expected_sha256 {
            return Err(crate::app::conversation_alignment::AlignmentError::StaleRecovery);
        }
        crate::app::conversation_alignment::replace_file(&temporary, path)
            .map_err(map_recovery_io)?;
        *replacement_landed = true;
        drop(source_guard);
        crate::app::conversation_alignment::sync_dir(parent).map_err(map_recovery_io)
    })();
    if let Err(error) = guarded_replace {
        let _ = fs::remove_file(&temporary);
        if keep_sidecar_backup && !*replacement_landed {
            let _ = fs::remove_file(&backup);
        }
        return Err(error);
    }
    Ok(keep_sidecar_backup.then_some(backup))
}

pub(crate) fn recovery_rollout_starts_with(
    path: &Path,
    expected_prefix: &[u8],
) -> Result<bool, crate::app::conversation_alignment::AlignmentError> {
    let metadata = fs::symlink_metadata(path).map_err(map_recovery_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    let mut source = fs::File::open(path).map_err(map_recovery_io)?;
    let mut buffer = [0_u8; 64 * 1024];
    for expected in expected_prefix.chunks(buffer.len()) {
        let mut consumed = 0;
        while consumed < expected.len() {
            let read = source
                .read(&mut buffer[consumed..expected.len()])
                .map_err(map_recovery_io)?;
            if read == 0 {
                return Ok(false);
            }
            consumed += read;
        }
        if buffer[..expected.len()] != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn stream_sha256(
    path: &Path,
) -> Result<[u8; 32], crate::app::conversation_alignment::AlignmentError> {
    let metadata = fs::symlink_metadata(path).map_err(map_recovery_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    let mut source = fs::File::open(path).map_err(map_recovery_io)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer).map_err(map_recovery_io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

pub(crate) fn replace_recovery_rollout_prefix(
    path: &Path,
    expected_prefix: &[u8],
    replacement_prefix: &[u8],
    replacement_landed: &mut bool,
    idle_check: &dyn Fn() -> Result<(), crate::app::conversation_alignment::AlignmentError>,
) -> Result<(), crate::app::conversation_alignment::AlignmentError> {
    *replacement_landed = false;
    let metadata = fs::symlink_metadata(path).map_err(map_recovery_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(crate::app::conversation_alignment::AlignmentError::BlockedMetadata);
    }
    let parent = path
        .parent()
        .ok_or(crate::app::conversation_alignment::AlignmentError::UnsafePath)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(crate::app::conversation_alignment::AlignmentError::UnsafePath)?;
    let temporary = path.with_file_name(format!(
        ".{filename}.desktop-prefix-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let permissions = metadata.permissions();
    let result = (|| {
        let mut source = open_replacement_guard(path).map_err(map_recovery_io)?;
        idle_check()?;
        let mut destination =
            crate::app::conversation_alignment::create_replacement_file(path, &temporary)
                .map_err(map_recovery_io)?;
        let mut source_hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        for expected in expected_prefix.chunks(buffer.len()) {
            let mut consumed = 0;
            while consumed < expected.len() {
                let read = source
                    .read(&mut buffer[consumed..expected.len()])
                    .map_err(map_recovery_io)?;
                if read == 0 {
                    return Err(crate::app::conversation_alignment::AlignmentError::StaleRecovery);
                }
                consumed += read;
            }
            if buffer[..expected.len()] != *expected {
                return Err(crate::app::conversation_alignment::AlignmentError::StaleRecovery);
            }
            source_hasher.update(expected);
        }
        destination
            .write_all(replacement_prefix)
            .map_err(map_recovery_io)?;
        let mut first_suffix_chunk = true;
        loop {
            let read = source.read(&mut buffer).map_err(map_recovery_io)?;
            if read == 0 {
                break;
            }
            source_hasher.update(&buffer[..read]);
            if first_suffix_chunk && !replacement_prefix.ends_with(b"\n") {
                destination.write_all(b"\n").map_err(map_recovery_io)?;
            }
            first_suffix_chunk = false;
            destination
                .write_all(&buffer[..read])
                .map_err(map_recovery_io)?;
        }
        destination.flush().map_err(map_recovery_io)?;
        destination.sync_all().map_err(map_recovery_io)?;
        drop(destination);
        preserve_source_permissions(&temporary, permissions).map_err(map_recovery_io)?;
        let copied_source_hash: [u8; 32] = source_hasher.finalize().into();
        if stream_sha256(path)? != copied_source_hash {
            return Err(crate::app::conversation_alignment::AlignmentError::StaleRecovery);
        }
        crate::app::conversation_alignment::replace_file(&temporary, path)
            .map_err(map_recovery_io)?;
        *replacement_landed = true;
        drop(source);
        crate::app::conversation_alignment::sync_dir(parent).map_err(map_recovery_io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(all(test, target_os = "windows"))]
fn upgrade_generated_recovery_rollout(
    path: &Path,
    expected_id: &str,
) -> Result<Option<PathBuf>, crate::app::conversation_alignment::AlignmentError> {
    let Some(prepared) = prepare_generated_recovery_upgrade(path, expected_id, None)? else {
        return Ok(None);
    };
    replace_recovery_rollout(
        path,
        &sha256_hex(&prepared.original),
        &prepared.upgraded,
        true,
        &mut false,
        &|| Ok(()),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        classify_existing_rollout, desktop_recovery_lines,
        generated_rollout_needs_desktop_upgrade_with_limits, map_recovery_io, read_recovery_bytes,
        ExistingRollout,
    };
    #[cfg(target_os = "windows")]
    use super::{replace_recovery_rollout, sha256_hex, upgrade_generated_recovery_rollout};
    use serde_json::{json, Value};
    use std::fs;
    #[cfg(target_os = "windows")]
    use std::{cell::Cell, fs::OpenOptions};

    fn event_types(lines: &[String]) -> Vec<(String, Option<String>)> {
        lines
            .iter()
            .skip(1)
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                (
                    value["type"].as_str().unwrap().to_string(),
                    value["payload"]["type"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    #[test]
    fn recovery_rollout_builds_desktop_turns_and_preserves_response_evidence() {
        let id = "019b5555-1111-7222-8333-444444444444";
        let messages = vec![
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]}),
            json!({"type":"function_call","name":"shell","arguments":"{}","call_id":"call-1"}),
            json!({"type":"function_call_output","call_id":"call-1","output":"real output"}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}),
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"second"}]}),
        ];

        let lines =
            desktop_recovery_lines(id, 1_700_000_000, "C:\\work", "openai", "1.0", &messages)
                .unwrap();

        assert_eq!(
            event_types(&lines),
            vec![
                ("event_msg".into(), Some("task_started".into())),
                ("event_msg".into(), Some("user_message".into())),
                ("response_item".into(), Some("message".into())),
                ("response_item".into(), Some("function_call".into())),
                ("response_item".into(), Some("function_call_output".into())),
                ("response_item".into(), Some("message".into())),
                ("event_msg".into(), Some("agent_message".into())),
                ("event_msg".into(), Some("task_complete".into())),
                ("event_msg".into(), Some("task_started".into())),
                ("event_msg".into(), Some("user_message".into())),
                ("response_item".into(), Some("message".into())),
                ("event_msg".into(), Some("task_complete".into())),
            ]
        );

        let values = lines
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let starts = values
            .iter()
            .filter(|v| v["payload"]["type"] == "task_started")
            .collect::<Vec<_>>();
        let completes = values
            .iter()
            .filter(|v| v["payload"]["type"] == "task_complete")
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 2);
        assert_eq!(completes.len(), 2);
        assert_eq!(
            starts[0]["payload"]["turn_id"],
            completes[0]["payload"]["turn_id"]
        );
        assert_eq!(
            starts[1]["payload"]["turn_id"],
            completes[1]["payload"]["turn_id"]
        );
        assert_eq!(
            values[0]["payload"]["originator"],
            "Codex history evidence recovery"
        );
        let response_payloads = values
            .iter()
            .filter(|value| value["type"] == "response_item")
            .map(|value| value["payload"].clone())
            .collect::<Vec<_>>();
        assert_eq!(response_payloads, messages);
    }

    #[test]
    fn image_only_user_evidence_builds_a_desktop_turn() {
        let id = "019b5555-1111-7222-8333-444444444444";
        let image_url = "data:image/png;base64,aW1hZ2U=";
        let messages = vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_image", "image_url": image_url, "detail": "high"}]
        })];

        let lines =
            desktop_recovery_lines(id, 1_700_000_000, "C:\\work", "openai", "1.0", &messages)
                .unwrap();
        let values = lines
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let user_event = values
            .iter()
            .find(|value| {
                value.pointer("/payload/type").and_then(Value::as_str) == Some("user_message")
            })
            .unwrap();

        assert_eq!(
            event_types(&lines)
                .iter()
                .filter(|(_, kind)| kind.as_deref() == Some("task_started"))
                .count(),
            1
        );
        assert_eq!(user_event["payload"]["message"], "");
        assert_eq!(user_event["payload"]["images"], json!([image_url]));
        assert_eq!(user_event["payload"]["audio"], json!([]));
        assert!(values
            .iter()
            .any(|value| { value["type"] == "response_item" && value["payload"] == messages[0] }));
    }

    #[test]
    fn audio_only_user_evidence_builds_a_desktop_turn() {
        let id = "019b5555-1111-7222-8333-444444444444";
        let audio_url = "data:audio/wav;base64,YXVkaW8=";
        let messages = vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_audio", "audio_url": audio_url}]
        })];

        let lines =
            desktop_recovery_lines(id, 1_700_000_000, "C:\\work", "openai", "1.0", &messages)
                .unwrap();
        let values = lines
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let user_event = values
            .iter()
            .find(|value| {
                value.pointer("/payload/type").and_then(Value::as_str) == Some("user_message")
            })
            .unwrap();

        assert_eq!(
            event_types(&lines)
                .iter()
                .filter(|(_, kind)| kind.as_deref() == Some("task_started"))
                .count(),
            1
        );
        assert_eq!(user_event["payload"]["message"], "");
        assert_eq!(user_event["payload"]["images"], json!([]));
        assert_eq!(user_event["payload"]["audio"], json!([audio_url]));
        assert!(values
            .iter()
            .any(|value| { value["type"] == "response_item" && value["payload"] == messages[0] }));
    }

    #[test]
    fn only_legacy_files_generated_by_this_tool_are_replaceable() {
        let id = "019b5555-1111-7222-8333-444444444444";
        let native =
            vec![json!({"type":"session_meta","payload":{"id":id,"originator":"codex_cli_rs"}})];
        let wrong_id = vec![
            json!({"type":"session_meta","payload":{"id":"019b6666-1111-7222-8333-444444444444","originator":"Codex history evidence recovery"}}),
        ];
        let legacy = vec![
            json!({"type":"session_meta","payload":{"id":id,"originator":"Codex history evidence recovery"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[]}}),
        ];
        let desktop = vec![
            json!({"type":"session_meta","payload":{"id":id,"originator":"Codex history evidence recovery"}}),
            json!({"type":"event_msg","payload":{"type":"task_started"}}),
            json!({"type":"event_msg","payload":{"type":"user_message"}}),
            json!({"type":"event_msg","payload":{"type":"task_complete"}}),
        ];

        assert_eq!(
            classify_existing_rollout(id, &native),
            ExistingRollout::Native
        );
        assert_eq!(
            classify_existing_rollout(id, &wrong_id),
            ExistingRollout::IdMismatch
        );
        assert_eq!(
            classify_existing_rollout(id, &legacy),
            ExistingRollout::GeneratedLegacy
        );
        assert_eq!(
            classify_existing_rollout(id, &desktop),
            ExistingRollout::GeneratedDesktopReadable
        );

        let interrupted = vec![
            json!({"type":"session_meta","payload":{"id":id,"originator":"Codex history evidence recovery"}}),
            json!({"type":"event_msg","payload":{"type":"task_started"}}),
            json!({"type":"event_msg","payload":{"type":"user_message"}}),
            json!({"type":"response_item","payload":{"type":"function_call","name":"shell"}}),
        ];
        assert_eq!(
            classify_existing_rollout(id, &interrupted),
            ExistingRollout::GeneratedDesktopReadable
        );

        let unrelated_event = vec![
            json!({"type":"session_meta","payload":{"id":id,"originator":"Codex history evidence recovery"}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{}}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"evidence"}]}}),
        ];
        assert_eq!(
            classify_existing_rollout(id, &unrelated_event),
            ExistingRollout::GeneratedLegacy
        );
    }

    #[test]
    fn recovery_reads_are_bounded_before_json_parsing() {
        let root =
            std::env::temp_dir().join(format!("cam-history-bounded-read-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout.jsonl");
        fs::write(&path, b"123456789").unwrap();

        assert!(read_recovery_bytes(&path, 8).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_planning_rejects_expansion_beyond_the_output_limit() {
        let root = std::env::temp_dir().join(format!(
            "cam-history-bounded-expansion-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let id = "019b5555-1111-7222-8333-444444444444";
        let path = root.join("rollout.jsonl");
        let records = [
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":id,"timestamp":"2026-01-01T00:00:00Z","cwd":"C:\\work","originator":"Codex history evidence recovery","cli_version":"1.0","model_provider":"openai"}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"x".repeat(512)}]}}),
        ];
        fs::write(
            &path,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();

        assert!(
            generated_rollout_needs_desktop_upgrade_with_limits(&path, id, 4096, 512,).is_err()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_planning_accepts_a_utf8_bom() {
        let root = std::env::temp_dir().join(format!("cam-history-bom-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let id = "019b5555-1111-7222-8333-444444444444";
        let path = root.join("rollout.jsonl");
        let records = [
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":id,"timestamp":"2026-01-01T00:00:00Z","cwd":"C:\\work","originator":"Codex history evidence recovery","cli_version":"1.0","model_provider":"openai"}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"evidence"}]}}),
        ];
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(
            (records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n")
                .as_bytes(),
        );
        fs::write(&path, bytes).unwrap();

        assert!(matches!(
            generated_rollout_needs_desktop_upgrade_with_limits(&path, id, 4096, 4096),
            Ok(true)
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn permission_denied_survives_the_recovery_io_boundary() {
        let error = map_recovery_io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));

        assert!(matches!(
            error,
            crate::app::conversation_alignment::AlignmentError::PermissionDenied
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn replacement_guard_denies_append_handles_through_the_idle_recheck() {
        let root = std::env::temp_dir().join(format!(
            "cam-history-replacement-guard-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout.jsonl");
        let original = b"original rollout\n";
        fs::write(&path, original).unwrap();
        let append_was_denied = Cell::new(false);
        let idle_check = || {
            append_was_denied.set(OpenOptions::new().append(true).open(&path).is_err());
            Ok(())
        };
        let mut landed = false;

        replace_recovery_rollout(
            &path,
            &sha256_hex(original),
            b"replacement rollout\n",
            false,
            &mut landed,
            &idle_check,
        )
        .unwrap();

        assert!(append_was_denied.get());
        assert!(landed);
        assert_eq!(fs::read(&path).unwrap(), b"replacement rollout\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn legacy_generated_rollout_is_backed_up_upgraded_and_idempotent() {
        let root =
            std::env::temp_dir().join(format!("cam-history-upgrade-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let id = "019b5555-1111-7222-8333-444444444444";
        let path = root.join(format!("rollout-2026-01-01T00-00-00-{id}.jsonl"));
        let records = vec![
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":id,"timestamp":"2026-01-01T00:00:00Z","cwd":"C:\\work","originator":"Codex history evidence recovery","cli_version":"1.0","model_provider":"openai"}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"evidence"}]}}),
        ];
        fs::write(
            &path,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();

        let backup = upgrade_generated_recovery_rollout(&path, id)
            .unwrap()
            .expect("legacy recovery should be upgraded");
        assert!(backup.exists());
        assert!(backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".pre-desktop-turns-"));
        let upgraded = fs::read_to_string(&path).unwrap();
        assert!(upgraded.contains("\"task_started\""));
        assert!(upgraded.contains("\"task_complete\""));
        assert!(upgraded.contains("\"user_message\""));
        assert!(upgrade_generated_recovery_rollout(&path, id)
            .unwrap()
            .is_none());

        let native_path = root.join(format!("native-{id}.jsonl"));
        fs::write(
            &native_path,
            json!({"type":"session_meta","payload":{"id":id,"originator":"codex_cli_rs"}})
                .to_string()
                + "\n",
        )
        .unwrap();
        assert!(upgrade_generated_recovery_rollout(&native_path, id)
            .unwrap()
            .is_none());
        assert_eq!(fs::read_to_string(&native_path).unwrap().lines().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
