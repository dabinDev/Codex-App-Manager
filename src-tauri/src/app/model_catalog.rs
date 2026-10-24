use serde::Deserialize;
use thiserror::Error;

pub const MAX_MODELS: usize = 256;
pub const MAX_MODEL_ID_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelCatalogError {
    #[error("the sidecar model response is invalid JSON")]
    InvalidJson,
    #[error("the sidecar model response has no models")]
    Empty,
    #[error("a sidecar model id is invalid")]
    InvalidModel,
    #[error("the sidecar returned too many models")]
    TooManyModels,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    pub models: Vec<String>,
    pub fetched_at_unix: u64,
    pub sidecar_version: String,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelRow>,
}

#[derive(Debug, Deserialize)]
struct ModelRow {
    id: String,
}

pub fn parse_model_catalog(bytes: &[u8]) -> Result<Vec<String>, ModelCatalogError> {
    let response: ModelsResponse =
        serde_json::from_slice(bytes).map_err(|_| ModelCatalogError::InvalidJson)?;
    if response.data.is_empty() {
        return Err(ModelCatalogError::Empty);
    }
    let mut models = response
        .data
        .into_iter()
        .map(|row| row.id.trim().to_owned())
        .collect::<Vec<_>>();
    if models.len() > MAX_MODELS {
        return Err(ModelCatalogError::TooManyModels);
    }
    for model in &models {
        if model.is_empty()
            || model.len() > MAX_MODEL_ID_BYTES
            || model.chars().any(char::is_control)
        {
            return Err(ModelCatalogError::InvalidModel);
        }
    }
    models.sort_unstable();
    models.dedup();
    if models.is_empty() {
        return Err(ModelCatalogError::Empty);
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deduplicates_and_sorts_openai_models() {
        let bytes =
            br#"{"data":[{"id":"gpt-z"},{"id":"gpt-a"},{"id":"gpt-z"},{"id":" claude-b "}]}"#;
        assert_eq!(
            parse_model_catalog(bytes).unwrap(),
            vec!["claude-b", "gpt-a", "gpt-z"]
        );
    }

    #[test]
    fn rejects_empty_and_control_character_model_ids() {
        assert!(matches!(
            parse_model_catalog(br#"{"data":[{"id":""}]}"#),
            Err(ModelCatalogError::InvalidModel)
        ));
        assert!(matches!(
            parse_model_catalog(br#"{"data":[{"id":"bad\nmodel"}]}"#),
            Err(ModelCatalogError::InvalidModel)
        ));
    }

    #[test]
    fn rejects_more_than_256_models() {
        let data = (0..257)
            .map(|index| serde_json::json!({ "id": format!("model-{index}") }))
            .collect::<Vec<_>>();
        let body = serde_json::json!({ "data": data });
        assert!(matches!(
            parse_model_catalog(&serde_json::to_vec(&body).unwrap()),
            Err(ModelCatalogError::TooManyModels)
        ));
    }
}
