use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::Mutex;

use super::model_catalog::{parse_model_catalog, ModelCatalog};

pub const OPEN_CODEX_BASE_URL: &str = "http://127.0.0.1:10100/v1";
const HEALTH_URL: &str = "http://127.0.0.1:10100/healthz";
const READY_URL: &str = "http://127.0.0.1:10100/readyz";
const MODELS_URL: &str = "http://127.0.0.1:10100/v1/models";
const KEY_ENV: &str = "CODEX_APP_MANAGER_OPENCODEX_KEY";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodexStatus {
    pub running: bool,
    pub ready: bool,
    pub port: u16,
    pub version: String,
}

#[derive(Debug, Error)]
pub enum OpenCodexError {
    #[error("OpenCodex runtime is unavailable")]
    RuntimeUnavailable,
    #[error("OpenCodex configuration could not be written")]
    ConfigIo,
    #[error("OpenCodex process could not be started")]
    Spawn,
    #[error("OpenCodex did not become ready")]
    NotReady,
    #[error("OpenCodex request failed")]
    Request,
    #[error("OpenCodex returned an invalid model list")]
    ModelCatalog(#[from] super::model_catalog::ModelCatalogError),
}

impl OpenCodexError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RuntimeUnavailable => "opencodex_runtime_unavailable",
            Self::ConfigIo => "opencodex_config_io",
            Self::Spawn => "opencodex_spawn_failed",
            Self::NotReady => "opencodex_not_ready",
            Self::Request => "opencodex_request_failed",
            Self::ModelCatalog(error) => match error {
                super::model_catalog::ModelCatalogError::InvalidJson => "opencodex_models_invalid",
                super::model_catalog::ModelCatalogError::Empty => "opencodex_models_empty",
                super::model_catalog::ModelCatalogError::InvalidModel => "opencodex_model_invalid",
                super::model_catalog::ModelCatalogError::TooManyModels => {
                    "opencodex_models_too_many"
                }
            },
        }
    }
}

struct Inner {
    runtime_root: PathBuf,
    client: reqwest::Client,
    child: Mutex<Option<Child>>,
    home: Mutex<Option<PathBuf>>,
    key_fingerprint: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct OpenCodexSupervisor(Arc<Inner>);

impl OpenCodexSupervisor {
    pub fn new(runtime_root: PathBuf) -> Self {
        Self(Arc::new(Inner {
            runtime_root,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(3))
                .build()
                .expect("OpenCodex HTTP client should build"),
            child: Mutex::new(None),
            home: Mutex::new(None),
            key_fingerprint: Mutex::new(None),
        }))
    }

    pub fn from_default_runtime() -> Self {
        let root = std::env::var_os("CODEX_APP_MANAGER_OPENCODEX_RUNTIME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe().ok().and_then(|path| {
                    path.parent()
                        .map(|parent| parent.join("resources/opencodex-runtime"))
                })
            })
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/opencodex-runtime")
            });
        Self::new(root)
    }

    pub async fn ensure_started(&self, api_key: &str) -> Result<OpenCodexStatus, OpenCodexError> {
        if api_key.trim().is_empty() {
            return Err(OpenCodexError::Request);
        }
        let key_fingerprint = key_fingerprint(api_key);
        let mut child = self.0.child.lock().await;
        if let Some(existing) = child.as_mut() {
            if existing
                .try_wait()
                .map_err(|_| OpenCodexError::Spawn)?
                .is_none()
            {
                let active_key = self.0.key_fingerprint.lock().await.clone();
                if active_key.as_deref() == Some(key_fingerprint.as_str()) {
                    drop(child);
                    self.wait_until_ready().await?;
                    return Ok(self.status(true, true).await);
                }
                terminate_child(existing);
                *child = None;
                *self.0.key_fingerprint.lock().await = None;
                if let Some(home) = self.0.home.lock().await.take() {
                    let _ = fs::remove_dir_all(home);
                }
            }
            *child = None;
        }

        let bun = runtime_file(&self.0.runtime_root, "bun.exe", "bun");
        let entry = self.0.runtime_root.join("package/bin/ocx.mjs");
        if !bun.is_file() || !entry.is_file() {
            return Err(OpenCodexError::RuntimeUnavailable);
        }
        let home = self.make_home()?;
        let upstream = format!(
            "{}/v1",
            super::brand::SERVICE_BASE_URL.trim_end_matches('/')
        );
        write_provider_config(&home, &upstream)?;
        let spawned = Command::new(&bun)
            .arg(&entry)
            .arg("start")
            .arg("--port")
            .arg("10100")
            .env("OPENCODEX_HOME", &home)
            .env(KEY_ENV, api_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| OpenCodexError::Spawn)?;
        *self.0.home.lock().await = Some(home);
        *self.0.key_fingerprint.lock().await = Some(key_fingerprint);
        *child = Some(spawned);
        drop(child);
        if let Err(error) = self.wait_until_ready().await {
            let _ = self.stop().await;
            return Err(error);
        }
        Ok(self.status(true, true).await)
    }

    pub async fn discover_models(&self, api_key: &str) -> Result<ModelCatalog, OpenCodexError> {
        self.ensure_started(api_key).await?;
        let response = self
            .0
            .client
            .get(MODELS_URL)
            .send()
            .await
            .map_err(|_| OpenCodexError::Request)?;
        if response.status() != StatusCode::OK {
            return Err(OpenCodexError::Request);
        }
        let body = response
            .bytes()
            .await
            .map_err(|_| OpenCodexError::Request)?;
        let models = parse_model_catalog(&body)?;
        Ok(ModelCatalog {
            models,
            fetched_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            sidecar_version: read_runtime_version(&self.0.runtime_root),
        })
    }

    pub async fn stop(&self) -> Result<OpenCodexStatus, OpenCodexError> {
        if let Some(mut child) = self.0.child.lock().await.take() {
            terminate_child(&mut child);
        }
        if let Some(home) = self.0.home.lock().await.take() {
            let _ = fs::remove_dir_all(home);
        }
        *self.0.key_fingerprint.lock().await = None;
        Ok(self.status(false, false).await)
    }

    async fn wait_until_ready(&self) -> Result<(), OpenCodexError> {
        for _ in 0..40 {
            let health = self.0.client.get(HEALTH_URL).send().await;
            let ready = self.0.client.get(READY_URL).send().await;
            if matches!(health, Ok(response) if response.status() == StatusCode::OK)
                && matches!(ready, Ok(response) if response.status() == StatusCode::OK)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(OpenCodexError::NotReady)
    }

    async fn status(&self, running: bool, ready: bool) -> OpenCodexStatus {
        OpenCodexStatus {
            running,
            ready,
            port: 10100,
            version: read_runtime_version(&self.0.runtime_root),
        }
    }

    fn make_home(&self) -> Result<PathBuf, OpenCodexError> {
        let root = super::paths::data_dir()
            .unwrap_or_else(|| std::env::temp_dir().join("codex-app-manager"))
            .join("opencodex");
        fs::create_dir_all(&root).map_err(|_| OpenCodexError::ConfigIo)?;
        Ok(root)
    }
}

impl Drop for OpenCodexSupervisor {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) != 1 {
            return;
        }
        if let Ok(mut guard) = self.0.child.try_lock() {
            if let Some(mut child) = guard.take() {
                terminate_child(&mut child);
            }
        }
    }
}

fn runtime_file(root: &Path, windows: &str, unix: &str) -> PathBuf {
    root.join(if cfg!(windows) { windows } else { unix })
}

fn key_fingerprint(api_key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(api_key.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn terminate_child(child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn write_provider_config(home: &Path, base_url: &str) -> Result<(), OpenCodexError> {
    let config = serde_json::json!({
        "providers": {
            "openai": {
                "adapter": "openai-responses",
                "baseUrl": base_url,
                "apiKey": format!("${{{KEY_ENV}}}"),
                "liveModels": true
            }
        }
    });
    fs::write(
        home.join("config.json"),
        serde_json::to_vec_pretty(&config).map_err(|_| OpenCodexError::ConfigIo)?,
    )
    .map_err(|_| OpenCodexError::ConfigIo)
}

fn read_runtime_version(root: &Path) -> String {
    fs::read(root.join("opencodex-runtime.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("version")
                .and_then(|version| version.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{write_provider_config, OpenCodexSupervisor};
    use std::fs;

    #[test]
    fn provider_config_references_environment_key_without_persisting_secret() {
        let home = std::env::temp_dir().join(format!("opencodex-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).unwrap();
        write_provider_config(&home, "http://127.0.0.1:10100/v1").unwrap();
        let text = fs::read_to_string(home.join("config.json")).unwrap();
        assert!(text.contains("${CODEX_APP_MANAGER_OPENCODEX_KEY}"));
        assert!(!text.contains("real-secret"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn supervisor_uses_a_private_runtime_root() {
        let supervisor = OpenCodexSupervisor::new(std::env::temp_dir().join("missing-opencodex"));
        assert_eq!(
            supervisor.0.runtime_root.file_name().unwrap(),
            "missing-opencodex"
        );
    }
}
