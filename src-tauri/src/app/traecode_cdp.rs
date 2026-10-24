use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use codex_theme_engine::cdp::{validated_debugger_url, CdpSession, TargetInfo};
use serde::Serialize;
use serde_json::{json, Value};
use sysinfo::{ProcessesToUpdate, System};
use thiserror::Error;

pub const TRAECODE_CDP_PORT: u16 = 9222;
pub const TRAECODE_IMPORT_SCRIPT: &str = include_str!("../../resources/traecode-model-import.js");
const TARGET_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const CALL_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_TARGET_LIST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraeCodeProbe {
    pub installed: bool,
    pub running: bool,
    pub cdp_ready: bool,
    pub restart_required: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TraeCodeError {
    #[error("TraeCode is not installed")]
    NotInstalled,
    #[error("TraeCode must be restarted with local debugging enabled")]
    RestartRequired,
    #[error("TraeCode could not be stopped")]
    StopFailed,
    #[error("TraeCode could not be launched")]
    LaunchFailed,
    #[error("TraeCode CDP is unavailable")]
    CdpUnavailable,
    #[error("TraeCode CDP request timed out")]
    CdpTimeout,
    #[error("TraeCode CDP evaluation failed")]
    CdpProtocol,
    #[error("TraeCode rejected model connectivity")]
    ConnectivityRejected,
    #[error("TraeCode custom model deletion failed")]
    DeleteFailed,
    #[error("TraeCode custom model creation failed")]
    AddFailed,
    #[error("TraeCode model list refresh failed")]
    RefreshFailed,
    #[error("TraeCode model services are unavailable")]
    ServiceUnavailable,
    #[error("another TraeCode model import is already running")]
    Busy,
    #[error("TraeCode integration is not supported on this platform")]
    UnsupportedPlatform,
}

impl TraeCodeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotInstalled => "traecode_not_installed",
            Self::RestartRequired => "traecode_restart_required",
            Self::StopFailed => "traecode_stop_failed",
            Self::LaunchFailed => "traecode_launch_failed",
            Self::CdpUnavailable => "traecode_cdp_unavailable",
            Self::CdpTimeout => "traecode_cdp_timeout",
            Self::CdpProtocol => "traecode_cdp_protocol",
            Self::ConnectivityRejected => "traecode_connectivity_rejected",
            Self::DeleteFailed => "traecode_delete_failed",
            Self::AddFailed => "traecode_add_failed",
            Self::RefreshFailed => "traecode_refresh_failed",
            Self::ServiceUnavailable => "traecode_service_unavailable",
            Self::Busy => "traecode_busy",
            Self::UnsupportedPlatform => "unsupported_platform",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TraeCodeEndpoint {
    pub executable: PathBuf,
    pub port: u16,
    pub target: TargetInfo,
    pub restarted: bool,
}

#[derive(Clone)]
pub struct CdpTransport {
    session: CdpSession,
}

impl CdpTransport {
    pub async fn connect(endpoint: &TraeCodeEndpoint) -> Result<Self, TraeCodeError> {
        let session = CdpSession::connect(endpoint.target.clone(), endpoint.port)
            .await
            .map_err(|_| TraeCodeError::CdpUnavailable)?;
        Ok(Self { session })
    }

    pub async fn call(
        &self,
        function: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, TraeCodeError> {
        self.call_with_timeout(function, arguments, CALL_TIMEOUT)
            .await
    }

    async fn call_with_timeout(
        &self,
        function: &str,
        arguments: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, TraeCodeError> {
        let context = tokio::time::timeout(
            timeout,
            self.session.send(
                "Runtime.evaluate",
                json!({
                    "expression": "globalThis",
                    "awaitPromise": false,
                    "returnByValue": false,
                    "userGesture": false,
                }),
            ),
        )
        .await
        .map_err(|_| TraeCodeError::CdpTimeout)?
        .map_err(|_| TraeCodeError::CdpProtocol)?;
        let object_id = context
            .pointer("/result/objectId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(TraeCodeError::CdpProtocol)?;
        let mut params = build_runtime_call(function, arguments);
        params["objectId"] = json!(object_id);
        let result =
            tokio::time::timeout(timeout, self.session.send("Runtime.callFunctionOn", params))
                .await
                .map_err(|_| TraeCodeError::CdpTimeout)?
                .map_err(|_| TraeCodeError::CdpProtocol)?;
        parse_runtime_call_result(result)
    }
}

pub async fn probe_traecode() -> Result<TraeCodeProbe, TraeCodeError> {
    #[cfg(not(target_os = "windows"))]
    {
        return Err(TraeCodeError::UnsupportedPlatform);
    }
    #[cfg(target_os = "windows")]
    {
        let running_paths = running_traecode_paths();
        let candidates = known_traecode_candidates();
        let selected = select_traecode_executable(&running_paths, &candidates, |path| {
            is_traecode_executable(path)
        });
        let running = !running_paths.is_empty();
        let cdp_ready = find_workbench_target(TRAECODE_CDP_PORT).await.is_ok();
        Ok(TraeCodeProbe {
            installed: selected.is_some(),
            running,
            cdp_ready,
            restart_required: selected.is_some() && running && !cdp_ready,
        })
    }
}

pub async fn ensure_traecode_cdp(
    restart_if_needed: bool,
) -> Result<TraeCodeEndpoint, TraeCodeError> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = restart_if_needed;
        return Err(TraeCodeError::UnsupportedPlatform);
    }
    #[cfg(target_os = "windows")]
    {
        let running_paths = running_traecode_paths();
        let candidates = known_traecode_candidates();
        let executable = select_traecode_executable(&running_paths, &candidates, |path| {
            is_traecode_executable(path)
        })
        .ok_or(TraeCodeError::NotInstalled)?;
        if let Ok(target) = find_workbench_target(TRAECODE_CDP_PORT).await {
            return Ok(TraeCodeEndpoint {
                executable,
                port: TRAECODE_CDP_PORT,
                target,
                restarted: false,
            });
        }

        let was_running = executable_is_running(&executable);
        if was_running && !restart_if_needed {
            return Err(TraeCodeError::RestartRequired);
        }
        if was_running {
            stop_traecode_processes(&executable).await?;
        }
        launch_traecode(&executable, TRAECODE_CDP_PORT)?;
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(target) = find_workbench_target(TRAECODE_CDP_PORT).await {
                return Ok(TraeCodeEndpoint {
                    executable,
                    port: TRAECODE_CDP_PORT,
                    target,
                    restarted: was_running,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(TraeCodeError::CdpUnavailable);
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    }
}

pub fn select_traecode_executable<F>(
    running: &[PathBuf],
    installed: &[PathBuf],
    is_file: F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    running
        .iter()
        .chain(installed)
        .find(|path| is_file(path))
        .cloned()
}

pub fn traecode_launch_arguments(port: u16) -> [String; 2] {
    [
        "--remote-debugging-address=127.0.0.1".to_owned(),
        format!("--remote-debugging-port={port}"),
    ]
}

pub fn select_workbench_target(
    targets: Vec<TargetInfo>,
    port: u16,
) -> Result<TargetInfo, TraeCodeError> {
    let mut targets = targets
        .into_iter()
        .filter(|target| {
            if target.target_type != "page" || validated_debugger_url(target, port).is_err() {
                return false;
            }
            let url = target.url.to_ascii_lowercase();
            let title = target.title.to_ascii_lowercase();
            url.starts_with("vscode-file://")
                && (url.contains("workbench.html")
                    || url.contains("trae")
                    || title.contains("trae"))
        })
        .collect::<Vec<_>>();
    targets.sort_by_key(|target| !target.url.to_ascii_lowercase().contains("workbench.html"));
    targets
        .into_iter()
        .next()
        .ok_or(TraeCodeError::CdpUnavailable)
}

pub fn build_runtime_call(function: &str, arguments: Vec<Value>) -> Value {
    let mut cdp_arguments = Vec::with_capacity(arguments.len() + 1);
    cdp_arguments.push(json!({ "value": function }));
    cdp_arguments.extend(arguments.into_iter().map(|value| json!({ "value": value })));
    json!({
        "functionDeclaration": TRAECODE_IMPORT_SCRIPT,
        "arguments": cdp_arguments,
        "awaitPromise": true,
        "returnByValue": true,
        "userGesture": true
    })
}

pub fn parse_runtime_call_result(result: Value) -> Result<Value, TraeCodeError> {
    if result.get("exceptionDetails").is_some_and(|details| {
        !details.is_null() && !details.as_object().is_some_and(|object| object.is_empty())
    }) {
        return Err(TraeCodeError::CdpProtocol);
    }
    Ok(result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}

pub fn sanitize_cdp_error(_message: &str) -> String {
    "TraeCode CDP evaluation failed".to_owned()
}

async fn find_workbench_target(port: u16) -> Result<TargetInfo, TraeCodeError> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(TARGET_TIMEOUT)
        .timeout(TARGET_TIMEOUT)
        .build()
        .map_err(|_| TraeCodeError::CdpUnavailable)?;
    let response = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
        .map_err(|_| TraeCodeError::CdpUnavailable)?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_TARGET_LIST_BYTES)
    {
        return Err(TraeCodeError::CdpUnavailable);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| TraeCodeError::CdpUnavailable)?;
    if bytes.len() as u64 > MAX_TARGET_LIST_BYTES {
        return Err(TraeCodeError::CdpUnavailable);
    }
    let targets = serde_json::from_slice::<Vec<TargetInfo>>(&bytes)
        .map_err(|_| TraeCodeError::CdpUnavailable)?;
    select_workbench_target(targets, port)
}

fn known_traecode_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("TRAE_EXE") {
        paths.push(PathBuf::from(path));
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(root)
                .join("Programs")
                .join("Trae CN")
                .join("Trae CN.exe"),
        );
    }
    if let Some(root) = std::env::var_os("ProgramFiles") {
        paths.push(PathBuf::from(root).join("Trae CN").join("Trae CN.exe"));
    }
    paths.push(PathBuf::from(r"D:\Trae CN\Trae CN.exe"));
    paths
}

fn is_traecode_executable(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("Trae CN.exe"))
        && path.is_file()
}

fn running_traecode_paths() -> Vec<PathBuf> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
        .processes()
        .values()
        .filter(|process| {
            process
                .name()
                .to_string_lossy()
                .eq_ignore_ascii_case("Trae CN.exe")
        })
        .filter_map(|process| process.exe().map(Path::to_path_buf))
        .filter(|path| is_traecode_executable(path))
        .collect()
}

fn executable_is_running(executable: &Path) -> bool {
    running_traecode_paths()
        .iter()
        .any(|path| same_executable_path(path, executable))
}

#[cfg(target_os = "windows")]
fn same_executable_path(left: &Path, right: &Path) -> bool {
    codex_win_engine::same_windows_path(left, right)
}

#[cfg(not(target_os = "windows"))]
fn same_executable_path(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(target_os = "windows")]
async fn stop_traecode_processes(executable: &Path) -> Result<(), TraeCodeError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let mut found = false;
        for process in system.processes().values() {
            if process
                .exe()
                .is_some_and(|path| same_executable_path(path, executable))
            {
                found = true;
                if !process.kill() {
                    return Err(TraeCodeError::StopFailed);
                }
            }
        }
        if !found {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(TraeCodeError::StopFailed);
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[cfg(target_os = "windows")]
fn launch_traecode(executable: &Path, port: u16) -> Result<(), TraeCodeError> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    Command::new(executable)
        .args(traecode_launch_arguments(port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|_| TraeCodeError::LaunchFailed)
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    fn target(title: &str, url: &str, websocket: &str) -> TargetInfo {
        TargetInfo {
            id: title.into(),
            title: title.into(),
            url: url.into(),
            target_type: "page".into(),
            web_socket_debugger_url: websocket.into(),
        }
    }

    #[test]
    fn installation_probe_prefers_the_running_verified_traecode_executable() {
        let running = [PathBuf::from(r"D:\Trae CN\Trae CN.exe")];
        let installed = [PathBuf::from(r"C:\Program Files\Trae CN\Trae CN.exe")];

        let selected = select_traecode_executable(&running, &installed, |path| {
            path == Path::new(r"D:\Trae CN\Trae CN.exe")
                || path == Path::new(r"C:\Program Files\Trae CN\Trae CN.exe")
        })
        .unwrap();

        assert_eq!(selected, PathBuf::from(r"D:\Trae CN\Trae CN.exe"));
    }

    #[test]
    fn launch_arguments_bind_cdp_to_loopback_only() {
        assert_eq!(
            traecode_launch_arguments(9222),
            [
                "--remote-debugging-address=127.0.0.1".to_string(),
                "--remote-debugging-port=9222".to_string(),
            ]
        );
    }

    #[test]
    fn runtime_result_accepts_a_null_exception_details_field() {
        let result = parse_runtime_call_result(json!({
            "exceptionDetails": null,
            "result": { "value": { "code": 0 } }
        }))
        .unwrap();

        assert_eq!(result, json!({ "code": 0 }));
    }

    #[test]
    fn runtime_result_accepts_an_empty_exception_details_object() {
        let result = parse_runtime_call_result(json!({
            "exceptionDetails": {},
            "result": { "value": { "code": 0 } }
        }))
        .unwrap();

        assert_eq!(result, json!({ "code": 0 }));
    }

    #[test]
    fn workbench_selection_rejects_unrelated_chromium_pages() {
        let targets = vec![
            target(
                "Fake workbench",
                "https://example.com/workbench.html",
                "ws://127.0.0.1:9222/devtools/page/fake",
            ),
            target(
                "Chrome",
                "https://example.com/",
                "ws://127.0.0.1:9222/devtools/page/chrome",
            ),
            target(
                "workspace",
                "vscode-file://vscode-app/d:/Trae%20CN/resources/app/out/vs/code/electron-sandbox/workbench/workbench.html",
                "ws://127.0.0.1:9222/devtools/page/trae",
            ),
        ];

        let selected = select_workbench_target(targets, 9222).unwrap();

        assert_eq!(selected.id, "workspace");
    }

    #[test]
    fn runtime_call_keeps_secrets_out_of_the_fixed_function_declaration() {
        let payload = build_runtime_call(
            "add",
            vec![json!({"model":"gpt-5.5","apiKey":"sk-secret-value"})],
        );
        let declaration = payload["functionDeclaration"].as_str().unwrap();

        assert_eq!(declaration, TRAECODE_IMPORT_SCRIPT);
        assert!(!declaration.contains("sk-secret-value"));
        assert_eq!(
            payload["arguments"][1]["value"]["apiKey"],
            "sk-secret-value"
        );
    }

    #[test]
    fn runtime_result_unwraps_values_and_rejects_renderer_exceptions() {
        assert_eq!(
            parse_runtime_call_result(json!({"result":{"value":{"code":0}}})).unwrap(),
            json!({"code":0})
        );
        assert_eq!(
            parse_runtime_call_result(json!({
                "exceptionDetails": {
                    "text": "failed for sk-secret-value"
                }
            }))
            .unwrap_err(),
            TraeCodeError::CdpProtocol
        );
    }

    #[test]
    fn cdp_errors_never_expose_api_keys() {
        let error = sanitize_cdp_error("evaluation failed for sk-secret-value");
        assert_eq!(error, "TraeCode CDP evaluation failed");
        assert!(!error.contains("sk-secret-value"));
    }

    async fn accept_transport() -> (
        CdpTransport,
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connect = tokio::spawn(async move {
            CdpTransport::connect(&TraeCodeEndpoint {
                executable: PathBuf::from(r"D:\Trae CN\Trae CN.exe"),
                port,
                target: target(
                    "workspace",
                    "vscode-file://vscode-app/d:/Trae%20CN/resources/app/out/vs/code/electron-sandbox/workbench/workbench.html",
                    &format!("ws://127.0.0.1:{port}/devtools/page/trae"),
                ),
                restarted: false,
            })
            .await
            .unwrap()
        });
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        for _ in 0..2 {
            let request = socket.next().await.unwrap().unwrap();
            let request: serde_json::Value =
                serde_json::from_str(request.to_text().unwrap()).unwrap();
            socket
                .send(Message::Text(
                    json!({"id": request["id"], "result": {}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        }
        (connect.await.unwrap(), socket)
    }

    #[tokio::test]
    async fn cdp_transport_matches_response_ids() {
        let (transport, mut socket) = accept_transport().await;
        let server = tokio::spawn(async move {
            let context_request = socket.next().await.unwrap().unwrap();
            let context_request: serde_json::Value =
                serde_json::from_str(context_request.to_text().unwrap()).unwrap();
            assert_eq!(context_request["method"], "Runtime.evaluate");
            socket
                .send(Message::Text(
                    json!({
                        "id": context_request["id"],
                        "result": { "result": { "objectId": "global-object" } }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();

            let request = socket.next().await.unwrap().unwrap();
            let request: serde_json::Value =
                serde_json::from_str(request.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "Runtime.callFunctionOn");
            assert_eq!(request["params"]["objectId"], "global-object");
            let id = request["id"].as_u64().unwrap();
            socket
                .send(Message::Text(
                    json!({"id": id + 100, "result": {"result":{"value":"wrong"}}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({"id": id, "result": {"result":{"value":"right"}}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });

        assert_eq!(transport.call("list", vec![]).await.unwrap(), "right");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cdp_transport_times_out_without_a_matching_response() {
        let (transport, mut socket) = accept_transport().await;
        let server = tokio::spawn(async move {
            let context_request = socket.next().await.unwrap().unwrap();
            let context_request: serde_json::Value =
                serde_json::from_str(context_request.to_text().unwrap()).unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "id": context_request["id"],
                        "result": { "result": { "objectId": "global-object" } }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let _request = socket.next().await.unwrap().unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        assert_eq!(
            transport
                .call_with_timeout("list", vec![], Duration::from_millis(20))
                .await
                .unwrap_err(),
            TraeCodeError::CdpTimeout
        );
        server.await.unwrap();
    }
}
