use std::collections::HashSet;
use std::path::Path;

use serde_json::json;
use thiserror::Error;

use crate::app::brand::SERVICE_NAME;

pub const CATALOG_NAME: &str = "app-manager-model-catalog.json";
const MANAGED_MARKER: &str = "# Codex App Manager managed setting";
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
    let models = normalize_model_selection(current)?;
    let default_model = models
        .first()
        .cloned()
        .ok_or(CodexModelConfigError::EmptySelection)?;
    Ok((models, default_model))
}

pub fn build_model_catalog(models: &[String]) -> Result<Vec<u8>, CodexModelConfigError> {
    let models = normalize_model_selection(models)?;
    let entries = models
        .iter()
        .enumerate()
        .map(|(index, slug)| {
            json!({
                "slug": slug,
                "display_name": slug,
                "description": format!("{slug} via {SERVICE_NAME}"),
                "shell_type": "unified_exec",
                "visibility": "list",
                "supported_in_api": true,
                "priority": index + 1,
                "base_instructions": "You are a helpful coding assistant.",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Fast responses"},
                    {"effort": "medium", "description": "Balanced reasoning"},
                    {"effort": "high", "description": "Deeper reasoning"},
                    {"effort": "xhigh", "description": "Maximum supported reasoning"}
                ],
                "default_reasoning_level": "high"
            })
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
    if !content.trim().is_empty() {
        toml::from_str::<toml::Value>(content).map_err(|_| CodexModelConfigError::InvalidConfig)?;
    }
    let eol = dominant_eol(content);
    let normalized = content.replace("\r\n", "\n");
    let mut lines = normalized
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let root_end = lines
        .iter()
        .position(|line| is_table_header(line))
        .unwrap_or(lines.len());
    let mut root_index = 0;
    lines.retain(|line| {
        let at_root = root_index < root_end;
        root_index += 1;
        if !at_root {
            return true;
        }
        let key = assignment_key(line);
        if line.trim() == MANAGED_MARKER
            || matches!(
                key,
                Some("openai_base_url" | "model_catalog_json" | "model")
            )
        {
            return false;
        }
        true
    });

    let first_table = lines.iter().position(|line| is_table_header(line));
    let mut insert_at = first_table.unwrap_or(lines.len());
    while insert_at > 0 && lines[insert_at - 1].trim().is_empty() {
        insert_at -= 1;
    }
    let managed = [
        MANAGED_MARKER.to_owned(),
        format!("openai_base_url = {}", toml_string(base_url)),
        format!("model_catalog_json = {}", toml_string(&catalog_path)),
        format!("model = {}", toml_string(default_model)),
        String::new(),
    ];
    lines.splice(insert_at..insert_at, managed);

    let merged = lines.join("\n");
    toml::from_str::<toml::Value>(&merged).map_err(|_| CodexModelConfigError::InvalidConfig)?;
    Ok(apply_eol(&merged, eol).into_bytes())
}

pub fn merge_codex_auth(
    existing: Option<&[u8]>,
    api_key: &str,
) -> Result<Vec<u8>, CodexModelConfigError> {
    if api_key.trim().is_empty() {
        return Err(CodexModelConfigError::EmptyKey);
    }
    let mut value = match existing {
        Some(bytes) => serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|_| CodexModelConfigError::InvalidAuth)?,
        None => json!({}),
    };
    let object = value
        .as_object_mut()
        .ok_or(CodexModelConfigError::InvalidAuth)?;
    object.insert("OPENAI_API_KEY".into(), json!(api_key));
    serde_json::to_vec_pretty(&value).map_err(|_| CodexModelConfigError::InvalidAuth)
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

fn assignment_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then_some(key)
}

fn is_table_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert!(text.contains("model_provider = \"OpenAI\""));
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
    fn auth_merge_replaces_only_the_openai_key() {
        let after = merge_codex_auth(
            Some(br#"{"OPENAI_API_KEY":"old","tokens":{"keep":true}}"#),
            "sk-new",
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&after).unwrap();

        assert_eq!(value["OPENAI_API_KEY"], "sk-new");
        assert_eq!(value["tokens"]["keep"], true);
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

        assert_eq!(models, vec!["claude-sonnet-4", "gpt-5.5"]);
        assert_eq!(default_model, "claude-sonnet-4");
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
