use std::collections::HashSet;
use std::path::Path;

use serde_json::json;
use thiserror::Error;

use crate::app::brand::SERVICE_NAME;

pub const CATALOG_NAME: &str = "app-manager-model-catalog.json";
pub const MANAGED_PROVIDER: &str = "app_manager";
const MAX_MODELS: usize = 256;
const MAX_MODEL_ID_BYTES: usize = 512;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodexModelConfigError {
    #[error("at least one model must be selected")]
    EmptySelection,
    #[error("too many models were selected")]
    TooManyModels,
    #[error("a selected model ID is invalid")]
    InvalidModel,
    #[error("selected model IDs must be unique")]
    DuplicateModel,
    #[error("the default model is not selected")]
    DefaultNotSelected,
    #[error("a selected model is no longer available for this API key")]
    ModelUnavailable,
    #[error("the Codex base URL is invalid")]
    InvalidBaseUrl,
    #[error("the Codex catalog path is invalid")]
    InvalidCatalogPath,
    #[error("the existing Codex configuration is invalid")]
    InvalidConfig,
    #[error("the existing Codex authentication file is invalid")]
    InvalidAuth,
    #[error("the API key is empty")]
    EmptyKey,
}

impl CodexModelConfigError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptySelection => "provider_models_empty",
            Self::TooManyModels => "provider_models_too_many",
            Self::InvalidModel => "provider_model_invalid",
            Self::DuplicateModel => "provider_model_duplicate",
            Self::DefaultNotSelected => "provider_default_not_selected",
            Self::ModelUnavailable => "provider_model_unavailable",
            Self::InvalidBaseUrl => "provider_base_url_invalid",
            Self::InvalidCatalogPath => "provider_catalog_path_invalid",
            Self::InvalidConfig => "provider_config_invalid",
            Self::InvalidAuth => "provider_auth_invalid",
            Self::EmptyKey => "provider_empty_key",
        }
    }
}

pub fn normalize_model_selection(models: &[String]) -> Result<Vec<String>, CodexModelConfigError> {
    if models.is_empty() {
        return Err(CodexModelConfigError::EmptySelection);
    }
    if models.len() > MAX_MODELS {
        return Err(CodexModelConfigError::TooManyModels);
    }
    let mut normalized = Vec::with_capacity(models.len());
    let mut seen = HashSet::with_capacity(models.len());
    for model in models {
        let model = model.trim();
        if model.is_empty()
            || model.len() > MAX_MODEL_ID_BYTES
            || model.chars().any(char::is_control)
        {
            return Err(CodexModelConfigError::InvalidModel);
        }
        if !seen.insert(model.to_owned()) {
            return Err(CodexModelConfigError::DuplicateModel);
        }
        normalized.push(model.to_owned());
    }
    Ok(normalized)
}

pub fn validate_codex_default(
    models: &[String],
    default_model: &str,
) -> Result<(), CodexModelConfigError> {
    let default_model = default_model.trim();
    if default_model.is_empty()
        || default_model.len() > MAX_MODEL_ID_BYTES
        || default_model.chars().any(char::is_control)
    {
        return Err(CodexModelConfigError::InvalidModel);
    }
    if !models.iter().any(|model| model == default_model) {
        return Err(CodexModelConfigError::DefaultNotSelected);
    }
    Ok(())
}

pub fn ensure_models_are_current(
    selected: &[String],
    current: &[String],
) -> Result<(), CodexModelConfigError> {
    if selected.iter().all(|model| current.contains(model)) {
        Ok(())
    } else {
        Err(CodexModelConfigError::ModelUnavailable)
    }
}

pub fn ensure_codex_models(models: &[String]) -> Result<(), CodexModelConfigError> {
    if models
        .iter()
        .all(|model| model.to_ascii_lowercase().starts_with("gpt-"))
    {
        Ok(())
    } else {
        Err(CodexModelConfigError::InvalidModel)
    }
}

pub fn all_models_with_default(
    current: &[String],
) -> Result<(Vec<String>, String), CodexModelConfigError> {
    let models = normalize_model_selection(&codex_models(current))?;
    let default_model = models
        .first()
        .cloned()
        .ok_or(CodexModelConfigError::EmptySelection)?;
    Ok((models, default_model))
}

pub fn codex_models(current: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    current
        .iter()
        .map(|model| model.trim())
        .filter(|model| model.to_ascii_lowercase().starts_with("gpt-"))
        .filter(|model| seen.insert((*model).to_owned()))
        .map(str::to_owned)
        .collect()
}

pub fn build_model_catalog(models: &[String]) -> Result<Vec<u8>, CodexModelConfigError> {
    let models = normalize_model_selection(models)?;
    let official: serde_json::Value =
        serde_json::from_str(include_str!("../../data/codex-models-0.164.0.json"))
            .expect("versioned official model catalog must be valid JSON");
    let entries = models
        .iter()
        .enumerate()
        .map(|(index, slug)| {
            let mut entry = official["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["slug"].as_str() == Some(slug.as_str()))
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "slug": slug,
                        "display_name": slug,
                        "description": format!("{slug} via {SERVICE_NAME}"),
                        "shell_type": "unified_exec",
                        "visibility": "list",
                        "supported_in_api": true,
                        "priority": index + 1,
                    "base_instructions": "You are a helpful coding assistant.",
                        "supported_reasoning_levels": [],
                        "default_reasoning_level": null,
                        "support_verbosity": false,
                        "truncation_policy": { "mode": "bytes", "limit": 10000 },
                        "experimental_supported_tools": [],
                        "input_modalities": ["text"]
                    })
                });
            // Only advertise selected, available models. Keep known model tool/prompt
            // metadata intact; unknown IDs use conservative optional capabilities.
            entry["visibility"] = json!("list");
            entry["supported_in_api"] = json!(true);
            entry["priority"] = json!(index + 1);
            entry["upgrade"] = serde_json::Value::Null;
            entry
        })
        .collect::<Vec<_>>();
    serde_json::to_vec_pretty(&json!({ "models": entries }))
        .map_err(|_| CodexModelConfigError::InvalidModel)
}

pub fn merge_codex_config(
    existing: Option<&[u8]>,
    base_url: &str,
    catalog_path: &Path,
    default_model: &str,
) -> Result<Vec<u8>, CodexModelConfigError> {
    validate_codex_base_url(base_url)?;
    if default_model.trim() != default_model
        || default_model.is_empty()
        || default_model.len() > MAX_MODEL_ID_BYTES
        || default_model.chars().any(char::is_control)
    {
        return Err(CodexModelConfigError::InvalidModel);
    }
    let catalog_path = catalog_path.to_string_lossy();
    if catalog_path.trim().is_empty() || catalog_path.chars().any(char::is_control) {
        return Err(CodexModelConfigError::InvalidCatalogPath);
    }

    let content = std::str::from_utf8(existing.unwrap_or_default())
        .map_err(|_| CodexModelConfigError::InvalidConfig)?;
    let eol = dominant_eol(content);
    let normalized = content.replace("\r\n", "\n");
    let mut document = normalized
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| CodexModelConfigError::InvalidConfig)?;
    let active_profile = document
        .get("profile")
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    set_model_settings(document.as_table_mut(), &catalog_path, default_model);
    document["openai_base_url"] = toml_edit::value(base_url);
    document["cli_auth_credentials_store"] = toml_edit::value("file");
    document["forced_login_method"] = toml_edit::value("api");
    if let Some(profile) = active_profile {
        let table = document
            .get_mut("profiles")
            .and_then(|item| item.get_mut(&profile))
            .and_then(|item| item.as_table_like_mut())
            .ok_or(CodexModelConfigError::InvalidConfig)?;
        set_model_settings(table, &catalog_path, default_model);
        // Codex 0.164 rejects the legacy root `profile` selector. Materialize
        // its supported settings without activating formerly ignored fields.
        let entries = table
            .iter()
            .filter(|(key, _)| {
                matches!(
                    *key,
                    "analytics"
                        | "approval_policy"
                        | "approvals_reviewer"
                        | "chatgpt_base_url"
                        | "experimental_compact_prompt_file"
                        | "experimental_use_unified_exec_tool"
                        | "features"
                        | "include_apps_instructions"
                        | "include_collaboration_mode_instructions"
                        | "include_environment_context"
                        | "include_environment_context_time"
                        | "include_permissions_instructions"
                        | "model"
                        | "model_catalog_json"
                        | "model_instructions_file"
                        | "model_provider"
                        | "oss_provider"
                        | "personality"
                        | "plan_mode_reasoning_effort"
                        | "sandbox_mode"
                        | "tools"
                        | "tui"
                        | "web_search"
                        | "windows"
                )
            })
            .map(|(key, item)| (key.to_owned(), item.clone()))
            .collect::<Vec<_>>();
        for (key, item) in entries {
            merge_profile_item(document.as_table_mut(), &key, item);
        }
        document.remove("profile");
    }
    let mut provider = toml_edit::Table::new();
    provider["name"] = toml_edit::value(SERVICE_NAME);
    provider["base_url"] = toml_edit::value(base_url);
    provider["wire_api"] = toml_edit::value("responses");
    provider["requires_openai_auth"] = toml_edit::value(true);
    provider["supports_websockets"] = toml_edit::value(false);
    if document.get("model_providers").is_none() {
        document["model_providers"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    document
        .get_mut("model_providers")
        .and_then(|item| item.as_table_like_mut())
        .ok_or(CodexModelConfigError::InvalidConfig)?
        .insert(MANAGED_PROVIDER, toml_edit::Item::Table(provider));
    let merged = document.to_string();
    toml::from_str::<toml::Value>(&merged).map_err(|_| CodexModelConfigError::InvalidConfig)?;
    Ok(apply_eol(&merged, eol).into_bytes())
}

fn merge_profile_item(table: &mut dyn toml_edit::TableLike, key: &str, item: toml_edit::Item) {
    if let Some(previous) = table.get_mut(key) {
        if let (Some(target), Some(source)) = (previous.as_table_like_mut(), item.as_table_like()) {
            for (child_key, child) in source.iter() {
                merge_profile_item(target, child_key, child.clone());
            }
        } else {
            *previous = item;
        }
    } else {
        table.insert(key, item);
    }
}

fn set_model_settings(table: &mut dyn toml_edit::TableLike, catalog: &str, model: &str) {
    set_string_preserving_comments(table, "model_provider", MANAGED_PROVIDER);
    set_string_preserving_comments(table, "model", model);
    set_string_preserving_comments(table, "model_catalog_json", catalog);
    // The selected model's metadata supplies its defaults, not the previous model.
    for key in [
        "model_reasoning_effort",
        "model_reasoning_summary",
        "model_verbosity",
        "service_tier",
    ] {
        table.remove(key);
    }
}

fn set_string_preserving_comments(table: &mut dyn toml_edit::TableLike, key: &str, value: &str) {
    if let Some(item) = table.get_mut(key) {
        if let Some(previous) = item.as_value_mut() {
            let decor = previous.decor().clone();
            *previous = toml_edit::Value::from(value);
            *previous.decor_mut() = decor;
            return;
        }
    }
    table.insert(key, toml_edit::value(value));
}

pub fn merge_codex_auth(
    existing: Option<&[u8]>,
    api_key: &str,
) -> Result<Vec<u8>, CodexModelConfigError> {
    if api_key.trim().is_empty() {
        return Err(CodexModelConfigError::EmptyKey);
    }
    let value = match existing {
        Some(bytes) => serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|_| CodexModelConfigError::InvalidAuth)?,
        None => json!({}),
    };
    if !value.is_object() {
        return Err(CodexModelConfigError::InvalidAuth);
    }
    // Match Codex login_with_api_key: do not retain another authentication mode.
    serde_json::to_vec_pretty(&json!({ "auth_mode": "apikey", "OPENAI_API_KEY": api_key }))
        .map_err(|_| CodexModelConfigError::InvalidAuth)
}

fn validate_codex_base_url(base_url: &str) -> Result<(), CodexModelConfigError> {
    let parsed = url::Url::parse(base_url).map_err(|_| CodexModelConfigError::InvalidBaseUrl)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/v1"
    {
        return Err(CodexModelConfigError::InvalidBaseUrl);
    }
    Ok(())
}

fn dominant_eol(content: &str) -> &'static str {
    let crlf = content.matches("\r\n").count();
    let bare_lf = content.matches('\n').count().saturating_sub(crlf);
    if crlf > 0 && crlf >= bare_lf {
        "\r\n"
    } else {
        "\n"
    }
}

fn apply_eol(content: &str, eol: &str) -> String {
    if eol == "\r\n" {
        content.replace('\n', "\r\n")
    } else {
        content.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_selects_managed_provider_in_active_profile_and_file_auth() {
        let before = br#"profile = "work"
model_provider = "previous"
cli_auth_credentials_store = "keyring"
forced_login_method = "chatgpt"
[profiles.work] # preserve active workflow
model_provider = "previous"
model = "old"
model_catalog_json = "C:/old.json"
sandbox_mode = "workspace-write"
[model_providers.previous]
name = "Previous"
base_url = "http://127.0.0.1:9/v1"
wire_api = "responses"
"#;
        let bytes = merge_codex_config(
            Some(before),
            "https://tomato.beinai.cc/v1",
            Path::new("C:/catalog.json"),
            "gpt-6.1-sol",
        )
        .unwrap();
        let config: toml::Value = toml::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert_eq!(config["model_provider"].as_str(), Some("app_manager"));
        assert!(config.get("profile").is_none());
        assert_eq!(config["sandbox_mode"].as_str(), Some("workspace-write"));
        assert_eq!(
            config["profiles"]["work"]["model_provider"].as_str(),
            Some("app_manager")
        );
        assert_eq!(
            config["profiles"]["work"]["model"].as_str(),
            Some("gpt-6.1-sol")
        );
        assert_eq!(
            config["profiles"]["work"]["model_catalog_json"].as_str(),
            Some("C:/catalog.json")
        );
        assert_eq!(
            config["profiles"]["work"]["sandbox_mode"].as_str(),
            Some("workspace-write")
        );
        assert_eq!(config["cli_auth_credentials_store"].as_str(), Some("file"));
        assert_eq!(config["forced_login_method"].as_str(), Some("api"));
        assert_eq!(
            config["model_providers"]["app_manager"]["base_url"].as_str(),
            Some("https://tomato.beinai.cc/v1")
        );
        assert_eq!(
            config["model_providers"]["previous"]["base_url"].as_str(),
            Some("http://127.0.0.1:9/v1")
        );
    }

    #[test]
    fn api_key_login_removes_conflicting_auth_material() {
        let existing = br#"{"auth_mode":"chatgpt","tokens":{},"last_refresh":"2026-01-01T00:00:00Z","personal_access_token":"old","agent_identity":{},"bedrock_api_key":"old"}"#;
        let bytes = merge_codex_auth(Some(existing), "sk-test-only").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            json!({"auth_mode":"apikey","OPENAI_API_KEY":"sk-test-only"})
        );
    }

    #[test]
    fn catalog_has_current_required_fields_without_inventing_unknown_model_reasoning() {
        let value: serde_json::Value = serde_json::from_slice(
            &build_model_catalog(&["gpt-6.1-sol".into(), "gpt-unknown-test".into()]).unwrap(),
        )
        .unwrap();
        for model in value["models"].as_array().unwrap() {
            assert!(model["support_verbosity"].is_boolean());
            assert!(model["truncation_policy"].is_object());
            assert!(model["experimental_supported_tools"].is_array());
        }
        assert_eq!(value["models"][0]["default_reasoning_level"], "low");
        assert_eq!(value["models"][1]["supported_reasoning_levels"], json!([]));
        assert!(value["models"][1]["default_reasoning_level"].is_null());
    }

    #[test]
    fn config_merge_preserves_unowned_tables_comments_and_crlf() {
        let before = b"# mine\r\nmodel = \"old\"\r\n\r\n[features]\r\ngoals = false\r\n";
        let after = merge_codex_config(
            Some(before),
            "https://tomato.beinai.cc/v1",
            Path::new("C:/Users/demo/.codex/app-manager-model-catalog.json"),
            "claude-sonnet-4",
        )
        .unwrap();
        let text = String::from_utf8(after).unwrap();

        assert!(text.contains("# mine\r\n"));
        assert!(text.contains("model = \"claude-sonnet-4\"\r\n"));
        assert!(text.contains("openai_base_url = \"https://tomato.beinai.cc/v1\"\r\n"));
        assert!(text.contains(
            "model_catalog_json = \"C:/Users/demo/.codex/app-manager-model-catalog.json\"\r\n"
        ));
        assert!(text.contains("[features]\r\ngoals = false\r\n"));
        assert!(!text.contains("[model_providers.OpenAI]"));
        assert!(!text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn config_merge_preserves_user_provider_tables_and_assignments() {
        let before = br#"model_provider = "OpenAI"
review_model = "keep-me"

[model_providers.OpenAI]
name = "OpenAI"
base_url = "https://tomato.beinai.cc/v1"
wire_api = "responses"
requires_openai_auth = true

[profiles.mine]
model_provider = "personal"
"#;
        let after = merge_codex_config(
            Some(before),
            "https://tomato.beinai.cc/v1",
            Path::new("C:/catalog.json"),
            "gpt-5.5",
        )
        .unwrap();
        let text = String::from_utf8(after).unwrap();

        assert!(text.contains("model_provider = \"app_manager\""));
        assert!(text.contains("[model_providers.OpenAI]"));
        assert!(text.contains("review_model = \"keep-me\""));
        assert!(text.contains("[profiles.mine]\nmodel_provider = \"personal\""));
    }

    #[test]
    fn config_merge_refuses_invalid_existing_toml() {
        assert!(merge_codex_config(
            Some(b"[features\ngoals = true"),
            "https://tomato.beinai.cc/v1",
            Path::new("C:/catalog.json"),
            "gpt-5.5",
        )
        .is_err());
    }

    #[test]
    fn auth_merge_replaces_previous_login_with_api_key_auth() {
        let after = merge_codex_auth(
            Some(br#"{"OPENAI_API_KEY":"old","tokens":{"keep":true}}"#),
            "sk-new",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&after).unwrap();

        assert_eq!(value["OPENAI_API_KEY"], "sk-new");
        assert_eq!(value["auth_mode"], "apikey");
        assert!(value.get("tokens").is_none());
    }

    #[test]
    fn auth_merge_refuses_invalid_existing_json() {
        assert!(merge_codex_auth(Some(br#"{"OPENAI_API_KEY":}"#), "sk-new").is_err());
    }

    #[test]
    fn model_selection_rejects_empty_duplicate_and_unselected_defaults() {
        assert!(normalize_model_selection(&[]).is_err());
        assert!(normalize_model_selection(&["gpt-5.5".into(), " gpt-5.5 ".into()]).is_err());
        let models =
            normalize_model_selection(&["claude-sonnet-4".into(), "gpt-5.5".into()]).unwrap();
        assert!(validate_codex_default(&models, "deepseek-v3").is_err());
        assert!(validate_codex_default(&models, "gpt-5.5").is_ok());
    }

    #[test]
    fn current_catalog_validation_rejects_models_that_disappeared() {
        let selected = vec!["claude-sonnet-4".into(), "gpt-5.5".into()];
        let current = vec!["gpt-5.5".into(), "deepseek-v3".into()];

        assert!(ensure_models_are_current(&selected, &current).is_err());
        assert!(ensure_models_are_current(&["gpt-5.5".into()], &current).is_ok());
    }

    #[test]
    fn codex_import_uses_the_complete_current_catalog_and_first_default() {
        let (models, default_model) =
            all_models_with_default(&["claude-sonnet-4".into(), "gpt-5.5".into()]).unwrap();

        assert_eq!(models, vec!["gpt-5.5"]);
        assert_eq!(default_model, "gpt-5.5");
        assert!(all_models_with_default(&["grok-3".into()]).is_err());
    }

    #[test]
    fn catalog_keeps_every_selected_slug_in_submission_order() {
        let json: serde_json::Value = serde_json::from_slice(
            &build_model_catalog(&["deepseek-v3".into(), "gpt-5.5".into()]).unwrap(),
        )
        .unwrap();

        assert_eq!(json["models"][0]["slug"], "deepseek-v3");
        assert_eq!(json["models"][0]["shell_type"], "unified_exec");
        assert_eq!(json["models"][0]["priority"], 1);
        assert_eq!(json["models"][1]["slug"], "gpt-5.5");
        assert_eq!(json["models"][1]["priority"], 2);
    }
}
