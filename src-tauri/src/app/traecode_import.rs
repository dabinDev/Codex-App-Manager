use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::traecode_cdp::{CdpTransport, TraeCodeError};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraeCodeModelRecord {
    pub id: String,
    pub model_id: String,
    pub display_name: String,
    pub personal_custom: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraeCodeCustomModel {
    pub model_id: String,
    pub display_name: String,
}

pub struct TraeCodeImportRequest {
    pub models: Vec<String>,
    pub restarted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraeCodeImportOutcome {
    Committed,
    Partial,
    FailedBeforeMutation,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraeCodeImportFailure {
    pub model_id: String,
    pub error_code: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraeCodeImportReport {
    pub outcome: TraeCodeImportOutcome,
    pub removed_count: usize,
    pub added_models: Vec<String>,
    pub failed_models: Vec<TraeCodeImportFailure>,
    pub restarted: bool,
    pub error_code: Option<String>,
}

pub(crate) trait TraeCodeModelTransport {
    async fn test_connectivity(&self, model: &TraeCodeCustomModel) -> Result<(), TraeCodeError>;
    async fn model_list_from_cache(&self) -> Result<Vec<TraeCodeModelRecord>, TraeCodeError>;
    async fn delete_custom_model(&self, model: &TraeCodeModelRecord) -> Result<(), TraeCodeError>;
    async fn add_custom_model(&self, model: &TraeCodeCustomModel) -> Result<(), TraeCodeError>;
    async fn refresh_model_store(&self) -> Result<(), TraeCodeError>;
}

pub(crate) trait TraeCodeScriptCaller {
    async fn call_script(
        &self,
        operation: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, TraeCodeError>;
}

impl TraeCodeScriptCaller for CdpTransport {
    async fn call_script(
        &self,
        operation: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, TraeCodeError> {
        self.call(operation, arguments).await
    }
}

pub(crate) struct TraeCodeCdpModelTransport<T = CdpTransport> {
    cdp: T,
    api_key: String,
    base_url: String,
}

impl<T> TraeCodeCdpModelTransport<T> {
    pub(crate) fn new(cdp: T, api_key: String, base_url: String) -> Self {
        Self {
            cdp,
            api_key,
            base_url,
        }
    }
}

impl<T: TraeCodeScriptCaller> TraeCodeModelTransport for TraeCodeCdpModelTransport<T> {
    async fn test_connectivity(&self, model: &TraeCodeCustomModel) -> Result<(), TraeCodeError> {
        let result = self
            .cdp
            .call_script(
                "test",
                vec![custom_model_payload(model, &self.api_key, &self.base_url)],
            )
            .await?;
        if service_operation_succeeded(&result) {
            Ok(())
        } else {
            Err(TraeCodeError::ConnectivityRejected)
        }
    }

    async fn model_list_from_cache(&self) -> Result<Vec<TraeCodeModelRecord>, TraeCodeError> {
        let result = self.cdp.call_script("list", Vec::new()).await?;
        parse_model_records(result)
    }

    async fn delete_custom_model(&self, model: &TraeCodeModelRecord) -> Result<(), TraeCodeError> {
        let result = self
            .cdp
            .call_script("delete", vec![json!({ "id": model.id })])
            .await?;
        if service_operation_succeeded(&result) {
            Ok(())
        } else {
            Err(TraeCodeError::DeleteFailed)
        }
    }

    async fn add_custom_model(&self, model: &TraeCodeCustomModel) -> Result<(), TraeCodeError> {
        let result = self
            .cdp
            .call_script(
                "add",
                vec![custom_model_payload(model, &self.api_key, &self.base_url)],
            )
            .await?;
        if service_operation_succeeded(&result) {
            Ok(())
        } else {
            Err(TraeCodeError::AddFailed)
        }
    }

    async fn refresh_model_store(&self) -> Result<(), TraeCodeError> {
        let result = self.cdp.call_script("refresh", Vec::new()).await?;
        if service_operation_succeeded(&result) {
            Ok(())
        } else {
            Err(TraeCodeError::RefreshFailed)
        }
    }
}

const DISPLAY_NAME_MAX_UTF16: usize = 32;
const TRAECODE_CUSTOM_MODEL_PREFIX: &str = "custom_openai_compatible//";

fn normalized_custom_model_id(model_id: &str) -> &str {
    model_id
        .strip_prefix(TRAECODE_CUSTOM_MODEL_PREFIX)
        .unwrap_or(model_id)
}

fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > max_units {
                false
            } else {
                units = next;
                true
            }
        })
        .collect()
}

fn build_custom_models(models: &[String]) -> Vec<TraeCodeCustomModel> {
    let mut used = HashSet::with_capacity(models.len());
    models
        .iter()
        .map(|model_id| {
            let mut display_name = truncate_utf16(model_id, DISPLAY_NAME_MAX_UTF16);
            let mut duplicate = 2;
            while !used.insert(display_name.clone()) {
                let suffix = format!("~{duplicate}");
                let prefix_units = DISPLAY_NAME_MAX_UTF16 - suffix.encode_utf16().count();
                display_name = format!("{}{}", truncate_utf16(model_id, prefix_units), suffix);
                duplicate += 1;
            }
            TraeCodeCustomModel {
                model_id: model_id.clone(),
                display_name,
            }
        })
        .collect()
}

fn custom_model_payload(model: &TraeCodeCustomModel, api_key: &str, base_url: &str) -> Value {
    json!({
        "model": model.model_id,
        "displayName": model.display_name,
        "apiKey": api_key,
        "baseUrl": base_url,
        "configDetail": { "multimodal": false },
    })
}

fn parse_model_records(value: Value) -> Result<Vec<TraeCodeModelRecord>, TraeCodeError> {
    let records = if value.is_array() {
        serde_json::from_value::<Vec<TraeCodeModelRecord>>(value)
            .map_err(|_| TraeCodeError::ServiceUnavailable)?
    } else {
        let groups = value.as_object().ok_or(TraeCodeError::ServiceUnavailable)?;
        let mut records = Vec::new();
        let mut seen = HashSet::new();
        for group in groups.values() {
            let models = group.as_array().ok_or(TraeCodeError::ServiceUnavailable)?;
            for model in models {
                if model.is_null() {
                    continue;
                }
                let model = model.as_object().ok_or(TraeCodeError::ServiceUnavailable)?;
                let personal_custom =
                    model.get("is_preset").and_then(Value::as_bool) == Some(false);
                let model_id = model
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(TraeCodeError::ServiceUnavailable)?
                    .to_owned();
                let id = model
                    .get("custom_model_id")
                    .and_then(|id| match id {
                        Value::String(value) => Some(value.clone()),
                        Value::Number(value) => Some(value.to_string()),
                        _ => None,
                    })
                    .filter(|id| !id.trim().is_empty())
                    .or_else(|| (!personal_custom).then(|| model_id.clone()))
                    .ok_or(TraeCodeError::ServiceUnavailable)?;
                if !seen.insert(id.clone()) {
                    continue;
                }
                let display_name = model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(&model_id)
                    .to_owned();
                records.push(TraeCodeModelRecord {
                    id,
                    model_id,
                    display_name,
                    personal_custom,
                });
            }
        }
        records
    };
    if records.iter().any(|record| {
        record.id.trim().is_empty()
            || record.model_id.trim().is_empty()
            || record.id.len() > 512
            || record.model_id.len() > 1024
            || record.display_name.len() > 1024
            || record.id.chars().any(char::is_control)
            || record.model_id.chars().any(char::is_control)
            || record.display_name.chars().any(char::is_control)
    }) {
        return Err(TraeCodeError::ServiceUnavailable);
    }
    Ok(records)
}

fn service_operation_succeeded(value: &Value) -> bool {
    value.get("code").and_then(Value::as_i64) == Some(0)
        || value.get("success").and_then(Value::as_bool) == Some(true)
        || value.get("status").and_then(Value::as_str) == Some("success")
        || value.get("status").and_then(Value::as_str) == Some("ok")
        || value.as_bool() == Some(true)
}

pub(crate) fn traecode_chat_completions_url(base_url: &str) -> String {
    format!("{}/v1/chat/completions", base_url.trim_end_matches('/'))
}

pub(crate) async fn import_traecode_models<T: TraeCodeModelTransport>(
    transport: &T,
    request: TraeCodeImportRequest,
) -> Result<TraeCodeImportReport, TraeCodeError> {
    let models = build_custom_models(&request.models);
    let selected_model_ids = models
        .iter()
        .map(|model| normalized_custom_model_id(model.model_id.as_str()))
        .collect::<HashSet<_>>();

    for model in &models {
        if let Err(error) = transport.test_connectivity(model).await {
            let error_code = error.code().to_owned();
            return Ok(TraeCodeImportReport {
                outcome: TraeCodeImportOutcome::FailedBeforeMutation,
                removed_count: 0,
                added_models: Vec::new(),
                failed_models: vec![TraeCodeImportFailure {
                    model_id: model.model_id.clone(),
                    error_code: error_code.clone(),
                }],
                restarted: request.restarted,
                error_code: Some(error_code),
            });
        }
    }

    let existing = transport.model_list_from_cache().await?;
    let mut removed_count = 0;
    for model in existing.iter().filter(|model| {
        model.personal_custom
            && selected_model_ids.contains(normalized_custom_model_id(model.model_id.as_str()))
    }) {
        match transport.delete_custom_model(model).await {
            Ok(()) => removed_count += 1,
            Err(error) => {
                if removed_count > 0 {
                    let _ = transport.refresh_model_store().await;
                }
                let error_code = error.code().to_owned();
                return Ok(TraeCodeImportReport {
                    outcome: if removed_count == 0 {
                        TraeCodeImportOutcome::FailedBeforeMutation
                    } else {
                        TraeCodeImportOutcome::Partial
                    },
                    removed_count,
                    added_models: Vec::new(),
                    failed_models: vec![TraeCodeImportFailure {
                        model_id: model.model_id.clone(),
                        error_code: error_code.clone(),
                    }],
                    restarted: request.restarted,
                    error_code: Some(error_code),
                });
            }
        }
    }

    let mut added_models = Vec::with_capacity(models.len());
    let mut failed_models = Vec::new();
    for model in &models {
        match transport.add_custom_model(model).await {
            Ok(()) => added_models.push(model.model_id.clone()),
            Err(error) => failed_models.push(TraeCodeImportFailure {
                model_id: model.model_id.clone(),
                error_code: error.code().to_owned(),
            }),
        }
    }
    let refresh_error = transport.refresh_model_store().await.err();
    let error_code = refresh_error
        .as_ref()
        .map(|error| error.code().to_owned())
        .or_else(|| {
            failed_models
                .first()
                .map(|failure| failure.error_code.clone())
        });

    Ok(TraeCodeImportReport {
        outcome: if failed_models.is_empty() && refresh_error.is_none() {
            TraeCodeImportOutcome::Committed
        } else {
            TraeCodeImportOutcome::Partial
        },
        removed_count,
        added_models,
        failed_models,
        restarted: request.restarted,
        error_code,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingState {
        models: Vec<TraeCodeModelRecord>,
        operations: Vec<String>,
        connectivity_failure: Option<String>,
        delete_failure: Option<String>,
        add_failure: Option<String>,
        refresh_failure: bool,
    }

    struct RecordingTraeCode {
        state: Mutex<RecordingState>,
    }

    impl RecordingTraeCode {
        fn with_custom_models(models: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                state: Mutex::new(RecordingState {
                    models: models
                        .into_iter()
                        .enumerate()
                        .map(|(index, model)| TraeCodeModelRecord {
                            id: format!("custom-{index}"),
                            model_id: model.to_owned(),
                            display_name: model.to_owned(),
                            personal_custom: true,
                        })
                        .collect(),
                    operations: Vec::new(),
                    connectivity_failure: None,
                    delete_failure: None,
                    add_failure: None,
                    refresh_failure: false,
                }),
            }
        }

        fn failing_connectivity(self, model: &str) -> Self {
            self.state.lock().unwrap().connectivity_failure = Some(model.to_owned());
            self
        }

        fn with_builtin_model(self, model: &str) -> Self {
            self.state.lock().unwrap().models.push(TraeCodeModelRecord {
                id: format!("builtin-{model}"),
                model_id: model.to_owned(),
                display_name: model.to_owned(),
                personal_custom: false,
            });
            self
        }

        fn failing_delete(self, model: &str) -> Self {
            self.state.lock().unwrap().delete_failure = Some(model.to_owned());
            self
        }

        fn failing_add(self, model: &str) -> Self {
            self.state.lock().unwrap().add_failure = Some(model.to_owned());
            self
        }

        fn failing_refresh(self) -> Self {
            self.state.lock().unwrap().refresh_failure = true;
            self
        }

        fn persisted_custom_models(&self) -> Vec<String> {
            self.state
                .lock()
                .unwrap()
                .models
                .iter()
                .filter(|model| model.personal_custom)
                .map(|model| model.model_id.clone())
                .collect()
        }

        fn operations(&self) -> Vec<String> {
            self.state.lock().unwrap().operations.clone()
        }

        fn persisted_models(&self) -> Vec<String> {
            self.state
                .lock()
                .unwrap()
                .models
                .iter()
                .map(|model| model.model_id.clone())
                .collect()
        }
    }

    impl TraeCodeModelTransport for RecordingTraeCode {
        async fn test_connectivity(
            &self,
            model: &TraeCodeCustomModel,
        ) -> Result<(), TraeCodeError> {
            let mut state = self.state.lock().unwrap();
            state.operations.push(format!("test:{}", model.model_id));
            if state.connectivity_failure.as_deref() == Some(model.model_id.as_str()) {
                Err(TraeCodeError::ConnectivityRejected)
            } else {
                Ok(())
            }
        }

        async fn model_list_from_cache(&self) -> Result<Vec<TraeCodeModelRecord>, TraeCodeError> {
            let mut state = self.state.lock().unwrap();
            state.operations.push("list".into());
            Ok(state.models.clone())
        }

        async fn delete_custom_model(
            &self,
            model: &TraeCodeModelRecord,
        ) -> Result<(), TraeCodeError> {
            let mut state = self.state.lock().unwrap();
            state.operations.push(format!("delete:{}", model.model_id));
            if state.delete_failure.as_deref() == Some(model.model_id.as_str()) {
                return Err(TraeCodeError::DeleteFailed);
            }
            state.models.retain(|candidate| candidate.id != model.id);
            Ok(())
        }

        async fn add_custom_model(&self, model: &TraeCodeCustomModel) -> Result<(), TraeCodeError> {
            let mut state = self.state.lock().unwrap();
            state.operations.push(format!("add:{}", model.model_id));
            if state.add_failure.as_deref() == Some(model.model_id.as_str()) {
                return Err(TraeCodeError::AddFailed);
            }
            state.models.push(TraeCodeModelRecord {
                id: format!("added-{}", model.model_id),
                model_id: model.model_id.clone(),
                display_name: model.display_name.clone(),
                personal_custom: true,
            });
            Ok(())
        }

        async fn refresh_model_store(&self) -> Result<(), TraeCodeError> {
            let mut state = self.state.lock().unwrap();
            state.operations.push("refresh".into());
            if state.refresh_failure {
                Err(TraeCodeError::RefreshFailed)
            } else {
                Ok(())
            }
        }
    }

    fn request(models: impl IntoIterator<Item = &'static str>) -> TraeCodeImportRequest {
        TraeCodeImportRequest {
            models: models.into_iter().map(str::to_owned).collect(),
            restarted: false,
        }
    }

    #[tokio::test]
    async fn failed_connectivity_preserves_every_existing_custom_model() {
        let transport =
            RecordingTraeCode::with_custom_models(["old-a", "old-b"]).failing_connectivity("new-b");

        let report = import_traecode_models(&transport, request(["new-a", "new-b"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::FailedBeforeMutation);
        assert_eq!(report.removed_count, 0);
        assert!(report.added_models.is_empty());
        assert_eq!(
            report.failed_models,
            vec![TraeCodeImportFailure {
                model_id: "new-b".into(),
                error_code: "traecode_connectivity_rejected".into(),
            }]
        );
        assert_eq!(
            report.error_code.as_deref(),
            Some("traecode_connectivity_rejected")
        );
        assert_eq!(transport.persisted_custom_models(), vec!["old-a", "old-b"]);
        assert_eq!(transport.operations(), vec!["test:new-a", "test:new-b"]);
    }

    #[tokio::test]
    async fn successful_import_preserves_unselected_personal_models_and_builtins() {
        let transport = RecordingTraeCode::with_custom_models(["old-a", "old-b"])
            .with_builtin_model("builtin-model");

        let report = import_traecode_models(&transport, request(["new-a", "new-b"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::Committed);
        assert_eq!(report.removed_count, 0);
        assert_eq!(report.added_models, vec!["new-a", "new-b"]);
        assert!(report.failed_models.is_empty());
        assert_eq!(report.error_code, None);
        assert_eq!(
            transport.persisted_models(),
            vec!["old-a", "old-b", "builtin-model", "new-a", "new-b"]
        );
        assert_eq!(
            transport.operations(),
            vec![
                "test:new-a",
                "test:new-b",
                "list",
                "add:new-a",
                "add:new-b",
                "refresh",
            ]
        );
    }

    #[tokio::test]
    async fn import_replaces_only_selected_custom_model_ids_and_preserves_other_custom_models() {
        let transport = RecordingTraeCode::with_custom_models([
            "gemini-2.5-pro",
            "custom_openai_compatible//grok-3",
        ]);

        let report = import_traecode_models(&transport, request(["grok-3"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::Committed);
        assert_eq!(report.removed_count, 1);
        assert_eq!(report.added_models, vec!["grok-3"]);
        assert!(report.failed_models.is_empty());
        assert_eq!(
            transport.persisted_custom_models(),
            vec!["gemini-2.5-pro", "grok-3"]
        );
        assert_eq!(
            transport.operations(),
            vec![
                "test:grok-3",
                "list",
                "delete:custom_openai_compatible//grok-3",
                "add:grok-3",
                "refresh",
            ]
        );
    }

    #[tokio::test]
    async fn deletion_failure_stops_before_additions_and_reports_prior_mutation() {
        let transport = RecordingTraeCode::with_custom_models(["old-a", "old-b"])
            .with_builtin_model("builtin-model")
            .failing_delete("old-b");

        let report = import_traecode_models(&transport, request(["old-a", "old-b"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::Partial);
        assert_eq!(report.removed_count, 1);
        assert!(report.added_models.is_empty());
        assert_eq!(
            report.failed_models,
            vec![TraeCodeImportFailure {
                model_id: "old-b".into(),
                error_code: "traecode_delete_failed".into(),
            }]
        );
        assert_eq!(report.error_code.as_deref(), Some("traecode_delete_failed"));
        assert_eq!(transport.persisted_models(), vec!["old-b", "builtin-model"]);
        assert_eq!(
            transport.operations(),
            vec![
                "test:old-a",
                "test:old-b",
                "list",
                "delete:old-a",
                "delete:old-b",
                "refresh",
            ]
        );
    }

    #[tokio::test]
    async fn addition_failure_does_not_skip_later_models_and_returns_partial_report() {
        let transport = RecordingTraeCode::with_custom_models(["old-a"]).failing_add("new-b");

        let report = import_traecode_models(&transport, request(["new-a", "new-b", "new-c"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::Partial);
        assert_eq!(report.removed_count, 0);
        assert_eq!(report.added_models, vec!["new-a", "new-c"]);
        assert_eq!(
            report.failed_models,
            vec![TraeCodeImportFailure {
                model_id: "new-b".into(),
                error_code: "traecode_add_failed".into(),
            }]
        );
        assert_eq!(report.error_code.as_deref(), Some("traecode_add_failed"));
        assert_eq!(
            transport.persisted_custom_models(),
            vec!["old-a", "new-a", "new-c"]
        );
        assert_eq!(
            transport.operations(),
            vec![
                "test:new-a",
                "test:new-b",
                "test:new-c",
                "list",
                "add:new-a",
                "add:new-b",
                "add:new-c",
                "refresh",
            ]
        );
    }

    #[test]
    fn display_names_are_deterministic_unique_and_bounded_by_utf16_units() {
        let models = vec![
            "12345678901234567890123456789012-alpha".to_owned(),
            "12345678901234567890123456789012-beta".to_owned(),
            format!("{}-tail", "😀".repeat(17)),
        ];

        let custom = build_custom_models(&models);
        let names = custom
            .iter()
            .map(|model| model.display_name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "12345678901234567890123456789012",
                "123456789012345678901234567890~2",
                "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀",
            ]
        );
        assert!(custom
            .iter()
            .all(|model| model.display_name.encode_utf16().count() <= 32));
        assert_eq!(custom[0].model_id, models[0]);
        assert_eq!(custom[1].model_id, models[1]);
        assert_eq!(custom[2].model_id, models[2]);
    }

    #[tokio::test]
    async fn refresh_failure_reports_mutated_state_instead_of_losing_the_result() {
        let transport = RecordingTraeCode::with_custom_models(["old-a"]).failing_refresh();

        let report = import_traecode_models(&transport, request(["old-a"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::Partial);
        assert_eq!(report.removed_count, 1);
        assert_eq!(report.added_models, vec!["old-a"]);
        assert!(report.failed_models.is_empty());
        assert_eq!(
            report.error_code.as_deref(),
            Some("traecode_refresh_failed")
        );
        assert_eq!(transport.persisted_custom_models(), vec!["old-a"]);
        assert_eq!(
            transport.operations(),
            vec!["test:old-a", "list", "delete:old-a", "add:old-a", "refresh"]
        );
    }

    #[test]
    fn cdp_arguments_keep_exact_model_and_credentials_in_structured_values() {
        let model = TraeCodeCustomModel {
            model_id: "vendor/model-v2".into(),
            display_name: "model-v2".into(),
        };

        let payload = custom_model_payload(
            &model,
            "sk-secret-value",
            "https://tomato.beinai.cc/v1/chat/completions",
        );

        assert_eq!(
            payload,
            serde_json::json!({
                "model": "vendor/model-v2",
                "displayName": "model-v2",
                "apiKey": "sk-secret-value",
                "baseUrl": "https://tomato.beinai.cc/v1/chat/completions",
                "configDetail": { "multimodal": false },
            })
        );
    }

    #[test]
    fn normalized_cache_records_parse_without_credential_fields() {
        let records = parse_model_records(serde_json::json!([
            {
                "id": "custom-42",
                "modelId": "custom_openai_compatible//old-a",
                "displayName": "Old A",
                "personalCustom": true
            },
            {
                "id": "builtin-a",
                "modelId": "builtin-a",
                "displayName": "Built in",
                "personalCustom": false
            }
        ]))
        .unwrap();

        assert_eq!(
            records,
            vec![
                TraeCodeModelRecord {
                    id: "custom-42".into(),
                    model_id: "custom_openai_compatible//old-a".into(),
                    display_name: "Old A".into(),
                    personal_custom: true,
                },
                TraeCodeModelRecord {
                    id: "builtin-a".into(),
                    model_id: "builtin-a".into(),
                    display_name: "Built in".into(),
                    personal_custom: false,
                },
            ]
        );
    }

    #[test]
    fn raw_agent_model_map_is_deduplicated_and_projects_no_credentials() {
        let records = parse_model_records(serde_json::json!({
            "builder": [
                {
                    "custom_model_id": "custom-42",
                    "name": "custom_openai_compatible//old-a",
                    "display_name": "Old A",
                    "is_preset": false,
                    "ak": "encrypted-secret",
                    "sk": "encrypted-secondary-secret"
                },
                {
                    "name": "builtin-a",
                    "display_name": "Built in",
                    "is_preset": true
                }
            ],
            "chat_v3": [
                {
                    "custom_model_id": "custom-42",
                    "name": "custom_openai_compatible//old-a",
                    "display_name": "Old A",
                    "is_preset": false,
                    "ak": "encrypted-secret"
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            records,
            vec![
                TraeCodeModelRecord {
                    id: "custom-42".into(),
                    model_id: "custom_openai_compatible//old-a".into(),
                    display_name: "Old A".into(),
                    personal_custom: true,
                },
                TraeCodeModelRecord {
                    id: "builtin-a".into(),
                    model_id: "builtin-a".into(),
                    display_name: "Built in".into(),
                    personal_custom: false,
                },
            ]
        );
        assert!(!format!("{records:?}").contains("encrypted-secret"));
    }

    #[test]
    fn service_operations_require_an_explicit_zero_code() {
        assert!(service_operation_succeeded(
            &serde_json::json!({ "code": 0 })
        ));
        assert!(!service_operation_succeeded(
            &serde_json::json!({ "code": 1 })
        ));
        assert!(!service_operation_succeeded(&serde_json::json!({})));
        assert!(!service_operation_succeeded(&Value::Null));
        assert!(!service_operation_succeeded(
            &serde_json::json!({ "code": "0" })
        ));
    }

    #[test]
    fn service_operations_accept_common_success_shapes() {
        assert!(service_operation_succeeded(
            &serde_json::json!({ "success": true })
        ));
        assert!(service_operation_succeeded(
            &serde_json::json!({ "status": "success" })
        ));
        assert!(service_operation_succeeded(&serde_json::json!(true)));
        assert!(!service_operation_succeeded(
            &serde_json::json!({ "success": false })
        ));
    }

    struct RecordingScript {
        calls: Mutex<Vec<(String, Vec<Value>)>>,
    }

    impl RecordingScript {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl TraeCodeScriptCaller for RecordingScript {
        async fn call_script(
            &self,
            operation: &str,
            arguments: Vec<Value>,
        ) -> Result<Value, TraeCodeError> {
            self.calls
                .lock()
                .unwrap()
                .push((operation.to_owned(), arguments));
            match operation {
                "list" => Ok(serde_json::json!([{
                    "id": "old-42",
                    "modelId": "vendor/new-a",
                    "displayName": "vendor/new-a",
                    "personalCustom": true
                }])),
                _ => Ok(serde_json::json!({ "code": 0 })),
            }
        }
    }

    #[tokio::test]
    async fn cdp_adapter_uses_fixed_operations_and_minimal_payloads() {
        let script = RecordingScript::new();
        let transport = TraeCodeCdpModelTransport::new(
            script,
            "sk-secret-value".into(),
            "https://tomato.beinai.cc/v1/chat/completions".into(),
        );

        let report = import_traecode_models(&transport, request(["vendor/new-a"]))
            .await
            .unwrap();

        assert_eq!(report.outcome, TraeCodeImportOutcome::Committed);
        let calls = transport.cdp.calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            [
                (
                    "test".into(),
                    vec![serde_json::json!({
                        "model": "vendor/new-a",
                        "displayName": "vendor/new-a",
                        "apiKey": "sk-secret-value",
                        "baseUrl": "https://tomato.beinai.cc/v1/chat/completions",
                        "configDetail": { "multimodal": false },
                    })],
                ),
                ("list".into(), vec![]),
                ("delete".into(), vec![serde_json::json!({ "id": "old-42" })]),
                (
                    "add".into(),
                    vec![serde_json::json!({
                        "model": "vendor/new-a",
                        "displayName": "vendor/new-a",
                        "apiKey": "sk-secret-value",
                        "baseUrl": "https://tomato.beinai.cc/v1/chat/completions",
                        "configDetail": { "multimodal": false },
                    })],
                ),
                ("refresh".into(), vec![]),
            ]
        );
    }

    #[test]
    fn traecode_request_url_uses_the_full_openai_endpoint() {
        assert_eq!(
            traecode_chat_completions_url("https://tomato.beinai.cc"),
            "https://tomato.beinai.cc/v1/chat/completions"
        );
        assert_eq!(
            traecode_chat_completions_url("https://token.cylonai.cn/"),
            "https://token.cylonai.cn/v1/chat/completions"
        );
    }
}
