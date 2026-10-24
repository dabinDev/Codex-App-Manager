use codex_theme_engine::locale::{
    build_zh_cn_payload, validate_runtime_report, LocaleRuntimeReport,
    CURRENT_LOCALE_REPORT_EXPRESSION, REMOVE_LOCALE_EXPRESSION,
};

#[test]
fn zh_cn_payload_is_embedded_versioned_and_complete() {
    let first = build_zh_cn_payload().expect("build locale payload");
    let second = build_zh_cn_payload().expect("build locale payload again");

    assert!(!first.payload.contains("__CAM_LOCALE_"));
    assert!(first.payload.contains("__camLocaleRuntimeV1"));
    assert!(first.payload.contains("data-cam-locale"));
    assert!(
        first.catalog_entries >= 80,
        "catalog should cover current Codex UI"
    );
    assert_eq!(
        first.stamp, second.stamp,
        "same runtime/catalog must have stable stamp"
    );
}

#[test]
fn locale_probe_and_removal_are_runtime_scoped() {
    assert!(CURRENT_LOCALE_REPORT_EXPRESSION.contains("__camLocaleRuntimeV1"));
    assert!(CURRENT_LOCALE_REPORT_EXPRESSION.contains("report"));
    assert!(REMOVE_LOCALE_EXPRESSION.contains("__camLocaleRuntimeV1"));
    assert!(REMOVE_LOCALE_EXPRESSION.contains("remove"));
    assert!(!REMOVE_LOCALE_EXPRESSION.contains("__CODEX_THEME_STUDIO__"));
}

#[test]
fn verified_runtime_requires_stamp_observer_catalog_and_visible_translation() {
    let payload = build_zh_cn_payload().unwrap();
    let valid = LocaleRuntimeReport {
        version: 1,
        stamp: payload.stamp.clone(),
        observer_active: true,
        catalog_entries: payload.catalog_entries,
        translated_nodes: 3,
    };
    validate_runtime_report(&payload, &valid).unwrap();

    let zero = LocaleRuntimeReport {
        translated_nodes: 0,
        ..valid.clone()
    };
    assert!(validate_runtime_report(&payload, &zero)
        .unwrap_err()
        .to_string()
        .contains("zero-translated-ui"));

    let stale = LocaleRuntimeReport {
        stamp: "old-runtime".to_string(),
        ..valid
    };
    assert!(validate_runtime_report(&payload, &stale).is_err());
}
