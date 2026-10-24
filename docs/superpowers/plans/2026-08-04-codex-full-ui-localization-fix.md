# Codex Full UI Localization Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Toolbox language action apply and verify Codex's official renderer locale instead of reporting success after only passing Chromium's `--lang` argument.

**Architecture:** Keep the existing persisted `codexLocale` and launch argument for native menus. Extend the existing loopback CDP settings adapter with one verified-setting operation, then add a `ThemeService` locale workflow that always restarts Codex in loopback-only CDP mode, preserves an active theme, writes `localeOverride`, and verifies that the Simplified Chinese catalog loaded. Expose that workflow through a dedicated Tauri command and frontend API.

**Tech Stack:** Rust 2021, Tokio, serde_json, Tauri v2, React 19, TypeScript 6, Vitest, Testing Library.

---

### Task 1: Add a verified Codex setting primitive

**Files:**
- Modify: `crates/codex-theme-engine/src/native_hot.rs`

- [ ] **Step 1: Write failing unit tests for verified values**

Add tests that call a new pure verifier with matching, mismatched, missing, and `null` values:

```rust
#[test]
fn verified_setting_accepts_string_and_null_values() {
    let mut string_values = serde_json::Map::new();
    string_values.insert("localeOverride".into(), serde_json::json!("zh-CN"));
    verify_setting_value("localeOverride", &serde_json::json!("zh-CN"), string_values).unwrap();

    let mut null_values = serde_json::Map::new();
    null_values.insert("localeOverride".into(), Value::Null);
    verify_setting_value("localeOverride", &Value::Null, null_values).unwrap();
}

#[test]
fn verified_setting_rejects_mismatch_and_missing_value() {
    let mut mismatch = serde_json::Map::new();
    mismatch.insert("localeOverride".into(), serde_json::json!("en-US"));
    assert!(verify_setting_value("localeOverride", &serde_json::json!("zh-CN"), mismatch)
        .unwrap_err().to_string().contains("setting-verification-failed"));
    assert!(verify_setting_value("localeOverride", &Value::Null, serde_json::Map::new())
        .unwrap_err().to_string().contains("setting-verification-failed"));
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --target-dir src-tauri/target native_hot::tests::verified_setting -- --nocapture`

Expected: compilation fails because `verify_setting_value` does not exist.

- [ ] **Step 3: Implement the verifier and one write/read operation**

Add a private verifier and a public asynchronous operation. The generated renderer expression must pass `{ key, default: null }`, preserving an explicit `null` so “follow system” can be verified:

```rust
fn verify_setting_value(
    key: &str,
    expected: &Value,
    mut values: serde_json::Map<String, Value>,
) -> Result<()> {
    match values.remove(key) {
        Some(actual) if actual == *expected => Ok(()),
        Some(actual) => Err(err(format!(
            "setting-verification-failed: {key}: expected {expected}, got {actual}"
        ))),
        None => Err(err(format!(
            "setting-verification-failed: {key}: read-back value missing"
        ))),
    }
}

pub async fn write_verified_value(
    session: &CdpSession,
    key: &str,
    expected: Value,
    version_hint: Option<&str>,
) -> Result<()> {
    ensure_api(session, version_hint).await?;
    let key_json = serde_json::to_string(key)
        .map_err(|e| err(format!("setting key serialization failed: {e}")))?;
    let expected_json = serde_json::to_string(&expected)
        .map_err(|e| err(format!("setting value serialization failed: {e}")))?;
    let expression = format!(r#"(async () => {{
  const api = window.__camThemeSettingsV1;
  if (!api) return {{ ok: false, error: "api not initialized" }};
  const key = {key_json};
  const expected = {expected_json};
  try {{
    await api.write({{ key }}, expected);
    const actual = await api.read({{ key, default: null }});
    return {{ ok: true, values: {{ [key]: actual }} }};
  }} catch (e) {{
    return {{ ok: false, error: key + ": " + String(e) }};
  }}
}})()"#);
    let outcome = run_js(session, &expression, "write and verify Codex setting").await?;
    verify_setting_value(key, &expected, outcome.values.unwrap_or_default())
}
```

- [ ] **Step 4: Run Task 1 tests and verify GREEN**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --target-dir src-tauri/target native_hot::tests -- --nocapture`

Expected: all `native_hot` tests pass, including string, `null`, mismatch, and missing-value cases.

### Task 2: Implement the restart, locale apply, and renderer verification workflow

**Files:**
- Modify: `crates/codex-theme-engine/src/native_hot.rs`
- Modify: `src-tauri/src/app/codex_theme.rs`

- [ ] **Step 1: Write failing tests for locale plans and renderer evidence**

Add a small locale plan type and pure verification helper tests to
`native_hot.rs`, alongside the settings adapter they describe:

```rust
#[test]
fn locale_plan_maps_chinese_and_system_default() {
    let zh = codex_locale_plan(Some("zh-CN"), Some("orange-dark")).unwrap();
    assert_eq!(zh.chromium_locale.as_deref(), Some("zh-CN"));
    assert_eq!(zh.setting_value, serde_json::json!("zh-CN"));
    assert!(zh.requires_catalog);
    assert_eq!(zh.active_theme.as_deref(), Some("orange-dark"));

    let system = codex_locale_plan(None, None).unwrap();
    assert_eq!(system.chromium_locale, None);
    assert_eq!(system.setting_value, Value::Null);
    assert!(!system.requires_catalog);
    assert_eq!(system.active_theme, None);
}

#[test]
fn locale_plan_rejects_unsupported_values() {
    assert!(codex_locale_plan(Some("en-US"), None).is_err());
    assert!(codex_locale_plan(Some("  "), None).is_err());
}

```

- [ ] **Step 2: Run focused theme-engine tests and verify RED**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --target-dir src-tauri/target native_hot::tests::locale_plan -- --nocapture`

Expected: compilation fails because `codex_locale_plan` and `has_locale_catalog_evidence` do not exist.

- [ ] **Step 3: Implement the pure plan and catalog matcher**

Add the public plan helper in `native_hot.rs`; `codex_theme.rs` consumes it and
maps engine failures to `AppError` at the orchestration boundary:

```rust
#[derive(Debug, Clone, PartialEq)]
struct CodexLocalePlan {
    chromium_locale: Option<String>,
    setting_value: serde_json::Value,
    requires_catalog: bool,
    active_theme: Option<String>,
}

pub fn codex_locale_plan(
    locale: Option<&str>,
    active_theme: Option<&str>,
) -> Result<CodexLocalePlan> {
    let active_theme = active_theme.map(str::to_owned);
    match locale {
        None => Ok(CodexLocalePlan {
            chromium_locale: None,
            setting_value: serde_json::Value::Null,
            requires_catalog: false,
            active_theme,
        }),
        Some("zh-CN") => Ok(CodexLocalePlan {
            chromium_locale: Some("zh-CN".to_string()),
            setting_value: serde_json::json!("zh-CN"),
            requires_catalog: true,
            active_theme,
        }),
        Some(value) => Err(err(format!(
            "codex-locale-unsupported: unsupported Codex locale {value}"
        ))),
    }
}

fn has_locale_catalog_evidence(locale: &str, resources: &[String]) -> bool {
    resources.iter().any(|url| {
        url.rsplit('/').next().is_some_and(|name| {
            name.starts_with(&format!("{locale}-")) && name.ends_with(".js")
        })
    })
}
```

- [ ] **Step 4: Add a CDP catalog-evidence operation**

Add these functions to `native_hot.rs`. Rust polls the current Renderer resource entries every 200 ms; a native-menu JSON file cannot satisfy the check:

```rust
pub fn has_locale_catalog_evidence(locale: &str, resources: &[String]) -> bool {
    resources.iter().any(|url| {
        url.rsplit('/').next().is_some_and(|name| {
            name.starts_with(&format!("{locale}-")) && name.ends_with(".js")
        })
    })
}

pub async fn wait_for_locale_catalog(
    session: &CdpSession,
    locale: &str,
    timeout: std::time::Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let value = session
            .evaluate(
                r#"performance.getEntriesByType("resource").map((entry) => entry.name)"#,
            )
            .await?;
        let resources: Vec<String> = serde_json::from_value(value)
            .map_err(|e| err(format!("locale catalog evidence parse failed: {e}")))?;
        if has_locale_catalog_evidence(locale, &resources) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(err(format!(
                "codex-locale-catalog-not-loaded: {locale} catalog did not load"
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}
```

Add the catalog-evidence unit test in `native_hot.rs`:

```rust
#[test]
fn chinese_catalog_evidence_requires_the_real_catalog_chunk() {
    assert!(has_locale_catalog_evidence(
        "zh-CN",
        &["app://-/webview/assets/zh-CN-ClxN0P7N.js".into()]
    ));
    assert!(!has_locale_catalog_evidence(
        "zh-CN",
        &["app://-/native-menu-locales/zh-CN.json".into()]
    ));
}
```

System default skips this check because the resolved system locale is not owned by the manager.

- [ ] **Step 5: Refactor the existing debuggable restart into one reusable helper**

Extract the stop/launch/CDP-ready block from `restart_debuggable_and_inject` into:

```rust
async fn restart_debuggable(&self, settings: &AppSettings) -> Result<(), AppError>
```

It must continue to call `launch_codex_with_cdp(..., THEME_CDP_PORT, ..., settings.codex_locale.as_deref())`, preserving `--remote-debugging-address=127.0.0.1`. Existing theme restart code calls this helper before native/CSS injection.

- [ ] **Step 6: Add `ThemeService::apply_locale_with_restart`**

The method builds the plan, always restarts to ensure native-menu arguments change, preserves an active theme through the existing restart injection path, connects only verified Codex renderer targets, writes and reads back `localeOverride`, waits for the `zh-CN-*.js` catalog when required, closes every CDP session, and returns success only after all checks pass:

```rust
pub async fn apply_locale_with_restart(
    &self,
    settings: &AppSettings,
) -> Result<(), AppError> {
    if !theme_supported() {
        return Err(AppError::UnsupportedPlatform);
    }
    ensure_no_pending_tx()?;
    let plan = codex_locale_plan(
        settings.codex_locale.as_deref(),
        settings.codex_theme.as_deref(),
    )?;
    let mut launch_settings = settings.clone();
    launch_settings.codex_locale = plan.chromium_locale.clone();

    if let Some(theme_ref) = plan.active_theme {
        self.restart_debuggable_and_inject(&launch_settings, &theme_ref, NativeSync::HotThenFile)
            .await?;
    } else {
        self.restart_debuggable(&launch_settings).await?;
    }

    let mut targets = codex_theme_engine::cdp::connect_codex_targets(
        THEME_CDP_PORT,
        Duration::from_secs(8),
    )
    .await
    .map_err(|e| AppError::Engine(e.to_string()))?;
    let session = targets.remove(0).session;
    for extra in targets {
        extra.session.close();
    }
    let version_hint = codex_version_hint();
    let outcome = async {
        native_hot::write_verified_value(
            &session,
            "localeOverride",
            plan.setting_value,
            version_hint.as_deref(),
        )
        .await
        .map_err(|e| AppError::Engine(e.to_string()))?;
        if plan.requires_catalog {
            native_hot::wait_for_locale_catalog(&session, "zh-CN", Duration::from_secs(10))
                .await
                .map_err(|e| AppError::Engine(e.to_string()))?;
        }
        Ok::<(), AppError>(())
    }
    .await;
    session.close();
    outcome
}
```

If an active theme cannot be reapplied, return the theme error rather than claiming localization succeeded with an inconsistent UI. Do not add an `app.asar` or WindowsApps fallback.

- [ ] **Step 7: Run Task 1 and Task 2 suites and verify GREEN**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --target-dir src-tauri/target native_hot::tests -- --nocapture`

Expected: all focused theme-engine tests pass without starting Codex or writing
real settings. The Tauri crate is covered by the later compile gate because the
locale command has no pure unit-test boundary of its own.

### Task 3: Expose a dedicated Tauri locale command

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/services/managerApi.ts`
- Modify: `src/services/managerApi.test.ts`

- [ ] **Step 1: Write the failing manager API contract test**

Extend the exact-command test to invoke the new API:

```ts
await managerApi.toolboxApplyCodexLocale(true);
expect(invokeMock).toHaveBeenCalledWith("toolbox_apply_codex_locale", {
  confirmed: true,
});
```

- [ ] **Step 2: Run the focused frontend API test and verify RED**

Run: `npx vitest run src/services/managerApi.test.ts --exclude "**/.worktrees/**"`

Expected: TypeScript/Vitest fails because `toolboxApplyCodexLocale` does not exist.

- [ ] **Step 3: Add the Tauri command**

Add `toolbox_apply_codex_locale(state, confirmed)` beside the generic force-restart command. Reuse `restart_command_policy` for explicit confirmation and busy errors, acquire the existing operation guard, load normalized persisted settings, and call:

```rust
state.codex_theme.apply_locale_with_restart(&settings).await
```

Register `commands::toolbox_apply_codex_locale` in `tauri::generate_handler!`. Keep stable command errors `alignment_restart_confirmation_required` and `alignment_busy`. Add a command-boundary mapper that translates the engine marker `codex-locale-catalog-not-loaded` to the stable frontend code `codex_locale_catalog_not_loaded`, while preserving the detailed message:

```rust
fn codex_locale_command_error(error: AppError) -> CommandError {
    let message = error.to_string();
    if message.contains("codex-locale-catalog-not-loaded") {
        return CommandError {
            code: "codex_locale_catalog_not_loaded".to_string(),
            message,
        };
    }
    error.into()
}
```

- [ ] **Step 4: Add the frontend API method**

```ts
toolboxApplyCodexLocale(confirmed: boolean): Promise<void> {
  if (!hasTauriRuntime()) return desktopRequired();
  return invoke<void>("toolbox_apply_codex_locale", { confirmed });
},
```

Update comments around `CODEX_LOCALE_OPTIONS` and `codexLocale` to describe the combined native-menu and official `localeOverride` behavior rather than claiming `--lang` controls the full React UI.

- [ ] **Step 5: Run API and Rust command checks and verify GREEN**

Run: `npx vitest run src/services/managerApi.test.ts --exclude "**/.worktrees/**"`

Run: `cargo check --manifest-path src-tauri/Cargo.toml --all-targets`

Expected: API contract test passes and the Tauri command compiles on the current host.

### Task 4: Make the Toolbox report only verified localization

**Files:**
- Create: `src/app/views/toolbox/UiLanguage.test.tsx`
- Modify: `src/app/views/toolbox/UiLanguage.tsx`
- Modify: `src/app/toolboxCopy.ts`

- [ ] **Step 1: Write failing UI tests**

Mock `getSettings`, `setSettings`, and the locale-specific API. Cover:

```ts
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../../services/managerApi";
import { DEFAULT_SETTINGS } from "../../../shared/types";
import { I18nProvider } from "../../i18n";
import { UiLanguage } from "./UiLanguage";

function renderLanguage() {
  return render(
    <I18nProvider>
      <UiLanguage />
    </I18nProvider>,
  );
}

describe("Codex UI language", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    vi.restoreAllMocks();
    vi.spyOn(managerApi, "getSettings").mockResolvedValue({ ...DEFAULT_SETTINGS });
    vi.spyOn(managerApi, "setSettings").mockImplementation(async (settings) => settings);
    vi.spyOn(managerApi, "toolboxApplyCodexLocale").mockResolvedValue(undefined);
  });

  it("applies Simplified Chinese through the verified locale command", async () => {
    const user = userEvent.setup();
    renderLanguage();
    await user.click(await screen.findByRole("radio", { name: "简体中文" }));
    await user.click(await screen.findByRole("button", { name: "Restart now" }));
    expect(managerApi.toolboxApplyCodexLocale).toHaveBeenCalledWith(true);
    expect(await screen.findByText("Codex UI language was applied and verified."))
      .toBeInTheDocument();
  });

  it("does not report success when renderer verification fails", async () => {
    const user = userEvent.setup();
    vi.mocked(managerApi.toolboxApplyCodexLocale).mockRejectedValue({
      code: "codex_locale_catalog_not_loaded",
    });
    renderLanguage();
    await user.click(await screen.findByRole("radio", { name: "简体中文" }));
    await user.click(await screen.findByRole("button", { name: "Restart now" }));
    expect(await screen.findByText(/did not load its Simplified Chinese catalog/))
      .toBeInTheDocument();
    expect(screen.queryByText("Codex UI language was applied and verified."))
      .not.toBeInTheDocument();
  });

  it("persists null and applies system default through the same command", async () => {
    const user = userEvent.setup();
    vi.mocked(managerApi.getSettings).mockResolvedValue({
      ...DEFAULT_SETTINGS,
      codexLocale: "zh-CN",
    });
    renderLanguage();
    await user.click(await screen.findByRole("radio", { name: "Follow system" }));
    await waitFor(() =>
      expect(managerApi.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({ codexLocale: null }),
      ),
    );
    await user.click(await screen.findByRole("button", { name: "Restart now" }));
    expect(managerApi.toolboxApplyCodexLocale).toHaveBeenCalledWith(true);
  });
});
```

- [ ] **Step 2: Run the UI test and verify RED**

Run: `npx vitest run src/app/views/toolbox/UiLanguage.test.tsx --exclude "**/.worktrees/**"`

Expected: tests fail because the component still calls `toolboxForceRestartCodex` and uses restart-only success copy.

- [ ] **Step 3: Switch `UiLanguage` to the dedicated command**

Rename `RestartStatus` to `ApplyStatus`, call `managerApi.toolboxApplyCodexLocale(true)`, and render success only after that promise resolves. Map `codex_locale_catalog_not_loaded` to a message explaining that the installed Codex/account did not load its Chinese UI catalog; preserve the generic and busy cases.

- [ ] **Step 4: Correct localized copy**

Update all locale dictionaries in `toolboxCopy.ts` so the feature describes full UI application and verification, not just launch arguments. Add `toolbox.locale.catalogUnavailable`; use precise English and Simplified Chinese source strings, while keeping the existing translated-object fallback pattern for other manager languages.

- [ ] **Step 5: Run all Toolbox tests and verify GREEN**

Run: `npx vitest run src/app/views/Toolbox.test.tsx src/app/views/toolbox/UiLanguage.test.tsx src/app/views/toolbox/ConversationAlignment.test.tsx --exclude "**/.worktrees/**"`

Expected: language tests pass and the generic conversation-alignment force restart remains unchanged.

### Task 5: Review, full verification, commit, and push

**Files:**
- Verify all modified files from Tasks 1-4

- [ ] **Step 1: Run formatting and static diff checks**

Run: `rustfmt --edition 2021 --check crates/codex-theme-engine/src/native_hot.rs crates/codex-win-engine/src/sys.rs src-tauri/src/app/codex_theme.rs src-tauri/src/app/settings_store.rs src-tauri/src/commands.rs src-tauri/src/domain/settings.rs src-tauri/src/lib.rs`

Run: `git diff --check`

Expected: touched Rust files and the final diff have no formatting or whitespace
errors. Do not bulk-format unrelated pre-existing repository differences.

- [ ] **Step 2: Run the repository-required review before commit**

Run: `codex review --uncommitted`

Expected: no actionable findings. Address each valid finding and rerun until clean.

- [ ] **Step 3: Run the full frontend quality gate**

Run: `npm run check`

Run: `npm run lint`

Run: `npx vitest run --exclude "**/.worktrees/**"`

Run: `npm run build`

Expected: brand synchronization, TypeScript, ESLint, all Vitest suites, and production build pass.

- [ ] **Step 4: Run full Rust verification**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --target-dir src-tauri/target --all-targets`

Run: `cargo test --manifest-path crates/codex-win-engine/Cargo.toml --target-dir src-tauri/target --all-targets`

Run: `cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --target-dir src-tauri/target --all-targets`

Run: `cargo test --manifest-path src-tauri/Cargo.toml --all-targets`

Run: `cargo check --manifest-path src-tauri/Cargo.toml --all-targets`

Expected: all independently buildable engine crates pass. Attempt both Tauri
commands and record the exact result. On this host, Windows application control
currently blocks the `aws-lc-sys` build script with OS error 4551; this is an
environmental verification gap, not permission to claim the Tauri crate passed.
No test launches or modifies the installed Codex app.

- [ ] **Step 5: Inspect the final diff and commit**

Run: `git status --short`

Run: `git diff --stat`

Run: `git diff`

Stage only the localization implementation, tests, copy, and plan. Commit:

```text
fix: apply and verify Codex renderer locale
```

- [ ] **Step 6: Push without force**

Run: `git push origin main`

Expected: the new implementation commit and the preceding design/plan commits are published to `origin/main`; never use force push.
