use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::conversation_alignment::{AlignmentOutcome, AlignmentReport};

const REQUEST_VERSION: u32 = 1;
const REQUEST_FILE_NAME: &str = "request.json";
const RESULT_FILE_NAME: &str = "result.json";
const ELEVATION_ROOT_NAME: &str = "alignment-elevation";
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_RESULT_BYTES: u64 = 256 * 1024;
const REQUEST_TTL_MS: u64 = 5 * 60 * 1_000;
const MAX_FUTURE_SKEW_MS: u64 = 30 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ElevationOperation {
    RestoreThenAlign,
    Align,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ElevationRequest {
    version: u32,
    operation: ElevationOperation,
    nonce: String,
    created_at_unix_ms: u64,
    codex_home: PathBuf,
    recovery_id: Option<String>,
    scan_token: String,
    result_file: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ValidatedElevationRequest {
    pub operation: ElevationOperation,
    pub nonce: String,
    pub codex_home: PathBuf,
    pub recovery_id: Option<String>,
    pub scan_token: String,
    pub result_file: PathBuf,
    pub operation_dir: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ElevationLaunch {
    pub request_path: PathBuf,
    pub result_file: PathBuf,
    pub operation_dir: PathBuf,
    pub nonce: String,
}

#[derive(Debug, Error)]
pub(crate) enum ElevationError {
    #[error("elevated alignment request is invalid")]
    InvalidRequest,
    #[error("elevated alignment request has expired")]
    Expired,
    #[error("elevated alignment request was already consumed")]
    Replayed,
    #[error("elevated alignment request could not be read")]
    Io,
    #[error("elevated alignment result is invalid")]
    InvalidResult,
    #[error("elevated alignment failed: {message}")]
    ElevatedFailure { code: String, message: String },
    #[error("elevation is unsupported on this platform")]
    Unsupported,
    #[error("elevation was cancelled")]
    Cancelled,
    #[error("elevated alignment timed out")]
    Timeout,
    #[error("elevated alignment process could not be started")]
    LaunchFailed,
}

impl ElevationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "alignment_elevation_invalid_request",
            Self::Expired => "alignment_elevation_expired",
            Self::Replayed => "alignment_elevation_replayed",
            Self::Io => "alignment_elevation_io",
            Self::InvalidResult => "alignment_elevation_invalid_result",
            Self::ElevatedFailure { .. } => "alignment_elevation_failed",
            Self::Unsupported => "alignment_elevation_unsupported",
            Self::Cancelled => "alignment_elevation_cancelled",
            Self::Timeout => "alignment_elevation_timeout",
            Self::LaunchFailed => "alignment_elevation_launch_failed",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ElevationResult {
    version: u32,
    nonce: String,
    report: Option<AlignmentReport>,
    error_code: Option<String>,
    error_message: Option<String>,
}

fn valid_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn safe_recovery_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ElevationError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| ElevationError::InvalidRequest)?;
    if !canonical.is_dir() {
        return Err(ElevationError::InvalidRequest);
    }
    Ok(canonical)
}

pub(crate) fn helper_request_path_from_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Option<PathBuf>, ElevationError> {
    let mut args = args.into_iter();
    let _executable = args.next().ok_or(ElevationError::InvalidRequest)?;
    let Some(flag) = args.next() else {
        return Ok(None);
    };
    if flag != "--alignment-elevated-helper" {
        return Ok(None);
    }
    let request = args.next().ok_or(ElevationError::InvalidRequest)?;
    if args.next().is_some() {
        return Err(ElevationError::InvalidRequest);
    }
    let request = PathBuf::from(request);
    if !request.is_absolute() {
        return Err(ElevationError::InvalidRequest);
    }
    Ok(Some(request))
}

pub(crate) fn prepare_elevation_request_at(
    app_state_root: &Path,
    codex_home: &Path,
    operation: ElevationOperation,
    recovery_id: Option<String>,
    scan_token: &str,
    created_at_unix_ms: u64,
) -> Result<ElevationLaunch, ElevationError> {
    if !valid_hex(scan_token, 64) {
        return Err(ElevationError::InvalidRequest);
    }
    match (operation, recovery_id.as_deref()) {
        (ElevationOperation::Align, None) => {}
        (ElevationOperation::RestoreThenAlign, Some(id)) if safe_recovery_id(id) => {}
        _ => return Err(ElevationError::InvalidRequest),
    }
    let app_state_root = canonical_directory(app_state_root)?;
    let codex_home = canonical_directory(codex_home)?;
    let elevation_root = app_state_root.join(ELEVATION_ROOT_NAME);
    if fs::symlink_metadata(&elevation_root)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(ElevationError::InvalidRequest);
    }
    fs::create_dir_all(&elevation_root).map_err(|_| ElevationError::Io)?;
    super::conversation_alignment::set_private_directory(&elevation_root)
        .map_err(|_| ElevationError::Io)?;
    let elevation_root = canonical_directory(&elevation_root)?;
    if elevation_root.parent() != Some(app_state_root.as_path()) {
        return Err(ElevationError::InvalidRequest);
    }

    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let operation_dir = elevation_root.join(format!("operation-{nonce}"));
    fs::create_dir(&operation_dir).map_err(|_| ElevationError::Io)?;
    if let Err(error) = super::conversation_alignment::set_private_directory(&operation_dir) {
        let _ = fs::remove_dir(&operation_dir);
        return Err(if error.kind() == std::io::ErrorKind::PermissionDenied {
            ElevationError::Io
        } else {
            ElevationError::InvalidRequest
        });
    }
    let operation_dir = canonical_directory(&operation_dir)?;
    let request_path = operation_dir.join(REQUEST_FILE_NAME);
    let result_file = operation_dir.join(RESULT_FILE_NAME);
    let request = ElevationRequest {
        version: REQUEST_VERSION,
        operation,
        nonce: nonce.clone(),
        created_at_unix_ms,
        codex_home,
        recovery_id,
        scan_token: scan_token.to_string(),
        result_file: PathBuf::from(RESULT_FILE_NAME),
    };
    let bytes = serde_json::to_vec(&request).map_err(|_| ElevationError::InvalidRequest)?;
    if let Err(error) = super::conversation_alignment::write_private_new(&request_path, &bytes) {
        let _ = fs::remove_dir_all(&operation_dir);
        return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
            ElevationError::InvalidRequest
        } else {
            ElevationError::Io
        });
    }
    super::conversation_alignment::sync_dir(&operation_dir).map_err(|_| ElevationError::Io)?;
    super::conversation_alignment::sync_dir(&elevation_root).map_err(|_| ElevationError::Io)?;

    Ok(ElevationLaunch {
        request_path,
        result_file,
        operation_dir,
        nonce,
    })
}

pub(crate) fn prepare_elevation_request(
    codex_home: &Path,
    operation: ElevationOperation,
    recovery_id: Option<String>,
    scan_token: &str,
) -> Result<ElevationLaunch, ElevationError> {
    let app_state = super::paths::data_dir().ok_or(ElevationError::Io)?;
    prepare_elevation_request_at(
        &app_state,
        codex_home,
        operation,
        recovery_id,
        scan_token,
        now_unix_ms(),
    )
}

pub(crate) fn cleanup_elevation_launch(launch: &ElevationLaunch) {
    let _ = fs::remove_dir_all(&launch.operation_dir);
}

pub(crate) fn collect_elevation_result(
    launch: &ElevationLaunch,
) -> Result<AlignmentReport, ElevationError> {
    let result = (|| {
        let metadata =
            fs::symlink_metadata(&launch.result_file).map_err(|_| ElevationError::InvalidResult)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_RESULT_BYTES
        {
            return Err(ElevationError::InvalidResult);
        }
        let canonical_result = launch
            .result_file
            .canonicalize()
            .map_err(|_| ElevationError::InvalidResult)?;
        if canonical_result.parent() != Some(launch.operation_dir.as_path()) {
            return Err(ElevationError::InvalidResult);
        }
        let mut bytes = Vec::new();
        fs::File::open(&canonical_result)
            .map_err(|_| ElevationError::InvalidResult)?
            .take(MAX_RESULT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ElevationError::InvalidResult)?;
        if bytes.len() as u64 > MAX_RESULT_BYTES {
            return Err(ElevationError::InvalidResult);
        }
        let result: ElevationResult =
            serde_json::from_slice(&bytes).map_err(|_| ElevationError::InvalidResult)?;
        if result.version != REQUEST_VERSION || result.nonce != launch.nonce {
            return Err(ElevationError::InvalidResult);
        }
        match (result.report, result.error_code, result.error_message) {
            (Some(report), None, None) => Ok(report),
            (None, Some(code), Some(message)) if !code.is_empty() && !message.is_empty() => {
                Err(ElevationError::ElevatedFailure { code, message })
            }
            _ => Err(ElevationError::InvalidResult),
        }
    })();
    cleanup_elevation_launch(launch);
    result
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn stop_helper_desktop() -> Result<bool, super::conversation_alignment::AlignmentError> {
    super::codex_theme::stop_for_external_config_write()
        .map_err(|_| super::conversation_alignment::AlignmentError::Mutation)
}

fn execute_validated_request(
    request: &ValidatedElevationRequest,
) -> Result<AlignmentReport, (String, String)> {
    let _provider_permit = super::codex_provider::begin_provider_write()
        .map_err(|error| (error.code().to_string(), error.to_string()))?;
    let paths =
        super::conversation_alignment::AlignmentPaths::from_codex_home(request.codex_home.clone());
    let process = super::conversation_alignment::SystemAlignmentProcessGate::new(
        super::codex_theme::installed_codex_path().ok(),
        stop_helper_desktop,
    );
    if request.operation == ElevationOperation::RestoreThenAlign {
        let recovery_id = request.recovery_id.as_deref().ok_or_else(|| {
            (
                "alignment_elevation_invalid_request".to_string(),
                "missing recovery identifier".to_string(),
            )
        })?;
        let restored = super::conversation_alignment::restore_alignment_with_process_at(
            &paths,
            recovery_id,
            &process,
        )
        .map_err(|error| (error.code().to_string(), error.to_string()))?;
        if restored.outcome != AlignmentOutcome::Restored || !restored.rollback_verified {
            return Err((
                restored
                    .error_code
                    .unwrap_or_else(|| "alignment_recovery_required".to_string()),
                "elevated restore could not verify the original state".to_string(),
            ));
        }
    }
    let report =
        super::conversation_alignment::execute_alignment_at(&paths, &request.scan_token, &process)
            .map_err(|error| (error.code().to_string(), error.to_string()))?;
    if report.elevation_required {
        return Err((
            report
                .error_code
                .clone()
                .unwrap_or_else(|| "alignment_permission_denied".to_string()),
            "elevated alignment still lacks permission".to_string(),
        ));
    }
    Ok(report)
}

fn write_helper_result(request: &ValidatedElevationRequest, result: ElevationResult) -> bool {
    let Ok(bytes) = serde_json::to_vec(&result) else {
        return false;
    };
    if bytes.len() as u64 > MAX_RESULT_BYTES {
        return false;
    }
    super::conversation_alignment::write_private_new(&request.result_file, &bytes).is_ok()
        && super::conversation_alignment::sync_dir(&request.operation_dir).is_ok()
}

fn run_helper_request(request_path: &Path) -> i32 {
    let Some(app_state) = super::paths::data_dir() else {
        return 2;
    };
    let Some(codex_home) = super::paths::codex_home_dir() else {
        return 2;
    };
    let request =
        match validate_and_consume_request_at(request_path, &app_state, &codex_home, now_unix_ms())
        {
            Ok(request) => request,
            Err(_) => return 2,
        };
    let result = match execute_validated_request(&request) {
        Ok(report) => ElevationResult {
            version: REQUEST_VERSION,
            nonce: request.nonce.clone(),
            report: Some(report),
            error_code: None,
            error_message: None,
        },
        Err((code, message)) => ElevationResult {
            version: REQUEST_VERSION,
            nonce: request.nonce.clone(),
            report: None,
            error_code: Some(code),
            error_message: Some(message),
        },
    };
    if write_helper_result(&request, result) {
        0
    } else {
        3
    }
}

pub fn run_helper_from_args(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    match helper_request_path_from_args(args) {
        Ok(None) => None,
        Ok(Some(request_path)) => Some(run_helper_request(&request_path)),
        Err(_) => Some(2),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn launch_elevated_helper(
    launch: &ElevationLaunch,
) -> Result<AlignmentReport, ElevationError> {
    cleanup_elevation_launch(launch);
    Err(ElevationError::Unsupported)
}

#[cfg(target_os = "windows")]
pub(crate) fn launch_elevated_helper(
    launch: &ElevationLaunch,
) -> Result<AlignmentReport, ElevationError> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    let executable = std::env::current_exe().map_err(|_| ElevationError::LaunchFailed)?;
    let verb = wide(std::ffi::OsStr::new("runas"));
    let executable = wide(executable.as_os_str());
    let parameters = wide(std::ffi::OsStr::new(&format!(
        "--alignment-elevated-helper \"{}\"",
        launch.request_path.display()
    )));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: null_mut(),
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: null(),
        nShow: SW_HIDE,
        hInstApp: null_mut(),
        lpIDList: null_mut(),
        lpClass: null(),
        hkeyClass: null_mut(),
        dwHotKey: 0,
        Anonymous: Default::default(),
        hProcess: null_mut(),
    };
    let launched = unsafe { ShellExecuteExW(&mut info) };
    if launched == 0 || info.hProcess.is_null() {
        let raw = std::io::Error::last_os_error().raw_os_error();
        cleanup_elevation_launch(launch);
        return Err(if raw == Some(ERROR_CANCELLED as i32) {
            ElevationError::Cancelled
        } else {
            ElevationError::LaunchFailed
        });
    }
    let wait = unsafe { WaitForSingleObject(info.hProcess, 5 * 60 * 1_000) };
    if wait == WAIT_TIMEOUT {
        unsafe {
            TerminateProcess(info.hProcess, 3);
            WaitForSingleObject(info.hProcess, 30_000);
            CloseHandle(info.hProcess);
        }
        cleanup_elevation_launch(launch);
        return Err(ElevationError::Timeout);
    }
    unsafe {
        CloseHandle(info.hProcess);
    }
    if wait != WAIT_OBJECT_0 {
        cleanup_elevation_launch(launch);
        return Err(ElevationError::LaunchFailed);
    }
    collect_elevation_result(launch)
}

pub(crate) fn validate_and_consume_request_at(
    request_path: &Path,
    app_state_root: &Path,
    approved_codex_home: &Path,
    now_unix_ms: u64,
) -> Result<ValidatedElevationRequest, ElevationError> {
    if !request_path.is_absolute()
        || request_path.file_name().and_then(|name| name.to_str()) != Some(REQUEST_FILE_NAME)
    {
        return Err(ElevationError::InvalidRequest);
    }

    let app_state_root = canonical_directory(app_state_root)?;
    let elevation_root = canonical_directory(&app_state_root.join(ELEVATION_ROOT_NAME))?;
    if elevation_root.parent() != Some(app_state_root.as_path()) {
        return Err(ElevationError::InvalidRequest);
    }
    let metadata =
        fs::symlink_metadata(request_path).map_err(|_| ElevationError::InvalidRequest)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_REQUEST_BYTES
    {
        return Err(ElevationError::InvalidRequest);
    }
    let canonical_request = request_path
        .canonicalize()
        .map_err(|_| ElevationError::InvalidRequest)?;
    let operation_dir = canonical_request
        .parent()
        .ok_or(ElevationError::InvalidRequest)?
        .to_path_buf();
    if operation_dir.parent() != Some(elevation_root.as_path())
        || !operation_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("operation-") && name.len() > 10)
    {
        return Err(ElevationError::InvalidRequest);
    }

    let mut bytes = Vec::new();
    fs::File::open(&canonical_request)
        .map_err(|_| ElevationError::Io)?
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ElevationError::Io)?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err(ElevationError::InvalidRequest);
    }
    let request: ElevationRequest =
        serde_json::from_slice(&bytes).map_err(|_| ElevationError::InvalidRequest)?;

    if request.version != REQUEST_VERSION
        || !valid_hex(&request.nonce, 32)
        || !valid_hex(&request.scan_token, 64)
        || request.created_at_unix_ms > now_unix_ms.saturating_add(MAX_FUTURE_SKEW_MS)
    {
        return Err(ElevationError::InvalidRequest);
    }
    if now_unix_ms.saturating_sub(request.created_at_unix_ms) > REQUEST_TTL_MS {
        return Err(ElevationError::Expired);
    }
    match (request.operation, request.recovery_id.as_deref()) {
        (ElevationOperation::Align, None) => {}
        (ElevationOperation::RestoreThenAlign, Some(id)) if safe_recovery_id(id) => {}
        _ => return Err(ElevationError::InvalidRequest),
    }

    let approved_codex_home = canonical_directory(approved_codex_home)?;
    if !request.codex_home.is_absolute()
        || canonical_directory(&request.codex_home)? != approved_codex_home
    {
        return Err(ElevationError::InvalidRequest);
    }
    let mut result_components = request.result_file.components();
    if !matches!(result_components.next(), Some(Component::Normal(name)) if name == RESULT_FILE_NAME)
        || result_components.next().is_some()
    {
        return Err(ElevationError::InvalidRequest);
    }
    let result_file = operation_dir.join(RESULT_FILE_NAME);
    if fs::symlink_metadata(&result_file)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(ElevationError::InvalidRequest);
    }

    let consumed = operation_dir.join(format!("nonce-{}.consumed", request.nonce));
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&consumed)
    {
        Ok(mut file) => {
            file.write_all(request.nonce.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| ElevationError::Io)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ElevationError::Replayed);
        }
        Err(_) => return Err(ElevationError::Io),
    }

    Ok(ValidatedElevationRequest {
        operation: request.operation,
        nonce: request.nonce,
        codex_home: approved_codex_home,
        recovery_id: request.recovery_id,
        scan_token: request.scan_token,
        result_file,
        operation_dir,
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{
        collect_elevation_result, helper_request_path_from_args, prepare_elevation_request_at,
        run_helper_from_args, validate_and_consume_request_at, AlignmentOutcome,
        ElevationOperation,
    };

    struct ElevationFixture {
        root: PathBuf,
        app_state: PathBuf,
        codex_home: PathBuf,
        operation_dir: PathBuf,
        request_path: PathBuf,
    }

    impl ElevationFixture {
        fn new(name: &str) -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-data")
                .join(format!(
                    "alignment-elevation-{name}-{}",
                    uuid::Uuid::new_v4()
                ));
            let app_state = root.join("app-state");
            let codex_home = root.join("codex-home");
            let operation_dir = app_state
                .join("alignment-elevation")
                .join(format!("operation-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&operation_dir).unwrap();
            fs::create_dir_all(&codex_home).unwrap();
            let request_path = operation_dir.join("request.json");
            let fixture = Self {
                root,
                app_state,
                codex_home,
                operation_dir,
                request_path,
            };
            fixture.write_request(json!({
                "version": 1,
                "operation": "align",
                "nonce": uuid::Uuid::new_v4().simple().to_string(),
                "createdAtUnixMs": now_ms(),
                "codexHome": fixture.codex_home,
                "recoveryId": null,
                "scanToken": "a".repeat(64),
                "resultFile": "result.json"
            }));
            fixture
        }

        fn write_request(&self, value: serde_json::Value) {
            fs::write(&self.request_path, serde_json::to_vec(&value).unwrap()).unwrap();
        }

        fn document(&self) -> serde_json::Value {
            serde_json::from_slice(&fs::read(&self.request_path).unwrap()).unwrap()
        }

        fn validate(&self) -> Result<super::ValidatedElevationRequest, super::ElevationError> {
            validate_and_consume_request_at(
                &self.request_path,
                &self.app_state,
                &self.codex_home,
                now_ms(),
            )
        }
    }

    impl Drop for ElevationFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    #[test]
    fn helper_accepts_a_valid_fixed_operation_request() {
        let fixture = ElevationFixture::new("valid");

        let request = fixture.validate().unwrap();

        assert_eq!(request.operation, ElevationOperation::Align);
        assert_eq!(
            request.codex_home,
            fixture.codex_home.canonicalize().unwrap()
        );
        assert_eq!(
            request.result_file,
            fixture
                .operation_dir
                .canonicalize()
                .unwrap()
                .join("result.json")
        );
    }

    #[test]
    fn parent_creates_a_request_inside_a_private_operation_directory() {
        let fixture = ElevationFixture::new("prepare");
        fs::remove_file(&fixture.request_path).unwrap();

        let launch = prepare_elevation_request_at(
            &fixture.app_state,
            &fixture.codex_home,
            ElevationOperation::Align,
            None,
            &"b".repeat(64),
            now_ms(),
        )
        .unwrap();

        assert!(launch.request_path.is_absolute());
        assert_eq!(launch.request_path.file_name().unwrap(), "request.json");
        assert_eq!(launch.result_file.file_name().unwrap(), "result.json");
        let request = validate_and_consume_request_at(
            &launch.request_path,
            &fixture.app_state,
            &fixture.codex_home,
            now_ms(),
        )
        .unwrap();
        assert_eq!(request.scan_token, "b".repeat(64));
    }

    #[test]
    fn helper_mode_accepts_only_the_exact_internal_argument_shape() {
        let absolute = std::env::current_dir().unwrap().join("request.json");
        let accepted = helper_request_path_from_args([
            OsString::from("manager.exe"),
            OsString::from("--alignment-elevated-helper"),
            absolute.clone().into_os_string(),
        ])
        .unwrap();
        assert_eq!(accepted, Some(absolute));

        assert!(helper_request_path_from_args([
            OsString::from("manager.exe"),
            OsString::from("--alignment-elevated-helper"),
            OsString::from("relative.json"),
        ])
        .is_err());
        assert!(helper_request_path_from_args([
            OsString::from("manager.exe"),
            OsString::from("--alignment-elevated-helper"),
            std::env::current_dir()
                .unwrap()
                .join("request.json")
                .into_os_string(),
            OsString::from("extra"),
        ])
        .is_err());
        assert_eq!(
            helper_request_path_from_args([OsString::from("manager.exe")]).unwrap(),
            None
        );
    }

    #[test]
    fn helper_mode_is_selected_before_tauri_only_for_the_internal_flag() {
        assert_eq!(run_helper_from_args([OsString::from("manager.exe")]), None);
        assert!(run_helper_from_args([
            OsString::from("manager.exe"),
            OsString::from("--alignment-elevated-helper"),
            OsString::from("relative.json"),
        ])
        .is_some_and(|code| code != 0));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_elevation_is_rejected_without_launching_a_process() {
        let fixture = ElevationFixture::new("unsupported");
        let launch = prepare_elevation_request_at(
            &fixture.app_state,
            &fixture.codex_home,
            ElevationOperation::Align,
            None,
            &"e".repeat(64),
            now_ms(),
        )
        .unwrap();

        let error = super::launch_elevated_helper(&launch).unwrap_err();

        assert_eq!(error.code(), "alignment_elevation_unsupported");
        assert!(!launch.operation_dir.exists());
    }

    #[test]
    fn parent_accepts_a_matching_structured_helper_result_and_cleans_up() {
        let fixture = ElevationFixture::new("result");
        let launch = prepare_elevation_request_at(
            &fixture.app_state,
            &fixture.codex_home,
            ElevationOperation::Align,
            None,
            &"c".repeat(64),
            now_ms(),
        )
        .unwrap();
        fs::write(
            &launch.result_file,
            serde_json::to_vec(&json!({
                "version": 1,
                "nonce": launch.nonce,
                "report": {
                    "outcome": "committed",
                    "recoveryId": null,
                    "backupDir": null,
                    "threadsUpdated": 2,
                    "rolloutsUpdated": 1,
                    "rolloutsSkipped": 1,
                    "writeVerified": true,
                    "rollbackVerified": false,
                    "codexWasRunning": false,
                    "restartAllowed": true,
                    "undoAllowed": true,
                    "elevationRequired": false,
                    "errorCode": null
                },
                "errorCode": null,
                "errorMessage": null
            }))
            .unwrap(),
        )
        .unwrap();

        let report = collect_elevation_result(&launch).unwrap();

        assert_eq!(report.outcome, AlignmentOutcome::Committed);
        assert_eq!(report.rollouts_skipped, 1);
        assert!(!launch.operation_dir.exists());
    }

    #[test]
    fn parent_rejects_a_helper_result_with_a_different_nonce() {
        let fixture = ElevationFixture::new("result-nonce");
        let launch = prepare_elevation_request_at(
            &fixture.app_state,
            &fixture.codex_home,
            ElevationOperation::Align,
            None,
            &"d".repeat(64),
            now_ms(),
        )
        .unwrap();
        fs::write(
            &launch.result_file,
            serde_json::to_vec(&json!({
                "version": 1,
                "nonce": "0".repeat(32),
                "report": null,
                "errorCode": "alignment_mutation_failed",
                "errorMessage": "failed"
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            collect_elevation_result(&launch).unwrap_err().code(),
            "alignment_elevation_invalid_result"
        );
    }

    #[test]
    fn helper_rejects_a_replayed_nonce() {
        let fixture = ElevationFixture::new("replay");
        fixture.validate().unwrap();

        let error = fixture.validate().unwrap_err();

        assert_eq!(error.code(), "alignment_elevation_replayed");
    }

    #[test]
    fn helper_rejects_unknown_operations() {
        let fixture = ElevationFixture::new("operation");
        let mut document = fixture.document();
        document["operation"] = json!("run_program");
        fixture.write_request(document);

        assert_eq!(
            fixture.validate().unwrap_err().code(),
            "alignment_elevation_invalid_request"
        );
    }

    #[test]
    fn helper_rejects_result_path_outside_private_operation_dir() {
        let fixture = ElevationFixture::new("result-path");
        let mut document = fixture.document();
        document["resultFile"] = json!("../../outside.json");
        fixture.write_request(document);

        assert_eq!(
            fixture.validate().unwrap_err().code(),
            "alignment_elevation_invalid_request"
        );
    }

    #[test]
    fn helper_rejects_expired_requests() {
        let fixture = ElevationFixture::new("expired");
        let mut document = fixture.document();
        document["createdAtUnixMs"] = json!(now_ms().saturating_sub(10 * 60 * 1_000));
        fixture.write_request(document);

        assert_eq!(
            fixture.validate().unwrap_err().code(),
            "alignment_elevation_expired"
        );
    }

    #[test]
    fn helper_rejects_an_invalid_scan_token() {
        let fixture = ElevationFixture::new("scan-token");
        let mut document = fixture.document();
        document["scanToken"] = json!("changed");
        fixture.write_request(document);

        assert_eq!(
            fixture.validate().unwrap_err().code(),
            "alignment_elevation_invalid_request"
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_rejects_a_symlinked_operation_directory_escape() {
        use std::os::unix::fs::symlink;

        let fixture = ElevationFixture::new("symlink");
        let outside = fixture.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let escaped = outside.join("request.json");
        fs::copy(&fixture.request_path, &escaped).unwrap();
        let link = fixture
            .app_state
            .join("alignment-elevation")
            .join("operation-linked");
        symlink(&outside, &link).unwrap();

        let error = validate_and_consume_request_at(
            &link.join("request.json"),
            &fixture.app_state,
            &fixture.codex_home,
            now_ms(),
        )
        .unwrap_err();

        assert_eq!(error.code(), "alignment_elevation_invalid_request");
    }

    #[test]
    fn helper_rejects_oversized_json() {
        let fixture = ElevationFixture::new("oversized");
        fs::write(&fixture.request_path, vec![b'x'; 70 * 1024]).unwrap();

        assert_eq!(
            fixture.validate().unwrap_err().code(),
            "alignment_elevation_invalid_request"
        );
    }

    #[test]
    fn helper_rejects_a_different_approved_codex_home() {
        let fixture = ElevationFixture::new("codex-home");
        let other = fixture.root.join("other-codex-home");
        fs::create_dir_all(&other).unwrap();

        let error = validate_and_consume_request_at(
            &fixture.request_path,
            &fixture.app_state,
            &other,
            now_ms(),
        )
        .unwrap_err();

        assert_eq!(error.code(), "alignment_elevation_invalid_request");
    }
}
