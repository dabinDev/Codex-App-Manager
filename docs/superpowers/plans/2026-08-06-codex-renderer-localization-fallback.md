# Codex Renderer Localization Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prefer Codex's official Simplified Chinese catalog, then install and verify a manager-owned renderer compatibility runtime when the account-level feature gate leaves the renderer in English.

**Architecture:** Add a locale payload module and a versioned JavaScript runtime beside the existing theme payload, then extend the loopback CDP daemon with an independent locale directive. `ThemeService` restarts Codex, verifies `localeOverride`, prefers the official catalog, falls back to the compatibility runtime, and returns a structured result consumed by the Toolbox UI.

**Tech Stack:** Rust 2021, Tokio watch channels, serde/serde_json, CDP, JavaScript DOM APIs, jsdom, Tauri v2, React 19, TypeScript 6, Vitest.

---

### Task 1: Build an independently authored locale catalog and payload

**Files:**
- Create: `crates/codex-theme-engine/src/runtime/locale-runtime.js`
- Create: `crates/codex-theme-engine/src/locale.rs`
- Create: `crates/codex-theme-engine/tests/locale_runtime.rs`
- Modify: `crates/codex-theme-engine/src/lib.rs`

- [ ] **Step 1: Write failing runtime contract tests**

Create jsdom-backed tests that load the runtime through `include_str!`, install a small test catalog, and assert exact text, `aria-label`, `title`, and `placeholder` replacement. Include protected fixtures for `[data-message-author-role]`, `pre`, `code`, `.diff`, `.terminal`, `.monaco-editor`, `input`, `textarea`, and `[contenteditable=true]` and assert their content is unchanged.

```rust
#[test]
fn runtime_translates_ui_and_preserves_user_content() {
    let report = run_fixture(Fixture::mixed_ui_and_content());
    assert_eq!(report.marker, "cam-locale-v1");
    assert!(report.observer_active);
    assert_eq!(report.translated_nodes, 4);
    assert_eq!(report.protected_text, "Review Settings Open Settings");
}

#[test]
fn runtime_reinjection_is_idempotent_and_observes_new_ui() {
    let report = run_fixture(Fixture::reinjected_with_dynamic_dialog());
    assert_eq!(report.observer_count, 1);
    assert_eq!(report.dynamic_dialog_text, "确认");
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --test locale_runtime -- --nocapture`

Expected: compilation fails because `locale` and `locale-runtime.js` do not exist.

- [ ] **Step 3: Implement the versioned runtime**

Implement `window.__camLocaleRuntimeV1` with one observer, exact string lookup, protected-ancestor checks, mutation batching through `queueMicrotask`, runtime-owned mutation suppression, and a `report()` method:

```js
window.__camLocaleRuntimeV1 = {
  version: 1,
  stamp,
  observer,
  catalogEntries: Object.keys(catalog).length,
  translatedNodes,
  install(nextCatalog, nextStamp) { /* update and rescan without a second observer */ },
  remove() { observer.disconnect(); document.documentElement.removeAttribute("data-cam-locale"); },
  report() { return { stamp, observerActive: true, catalogEntries, translatedNodes }; },
};
```

Use exact entries for current Codex navigation, composer, settings, tools, account, confirmations, updates, and error surfaces. Do not include regex replacement or content-derived telemetry.

- [ ] **Step 4: Implement locale payload assembly**

Expose an embedded payload and probe expressions:

```rust
pub struct LocalePayload {
    pub payload: String,
    pub stamp: String,
    pub catalog_entries: usize,
}

pub fn build_zh_cn_payload() -> Result<LocalePayload>;
pub const CURRENT_LOCALE_REPORT_EXPRESSION: &str;
pub const REMOVE_LOCALE_EXPRESSION: &str;
```

Hash runtime and catalog bytes into the stamp and serialize the catalog with `serde_json`; never interpolate hand-escaped JavaScript strings.

- [ ] **Step 5: Run Task 1 tests and verify GREEN**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --test locale_runtime -- --nocapture`

Expected: all exact-replacement, protected-content, mutation, and idempotency tests pass.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/codex-theme-engine/src/runtime/locale-runtime.js crates/codex-theme-engine/src/locale.rs crates/codex-theme-engine/src/lib.rs crates/codex-theme-engine/tests/locale_runtime.rs
git commit -m "feat: add Codex renderer locale runtime"
```

### Task 2: Make theme and locale daemon directives independent

**Files:**
- Modify: `crates/codex-theme-engine/src/daemon.rs`
- Modify: `src-tauri/src/app/codex_theme.rs`

- [ ] **Step 1: Write failing daemon state tests**

Add pure reconciliation tests for a new directive object:

```rust
#[test]
fn changing_theme_does_not_clear_locale() {
    let state = Directive { theme_dir: None, locale: Some(LocaleDirective::ZhCn) };
    let changed = state.with_theme(Some(PathBuf::from("orange")));
    assert_eq!(changed.locale, Some(LocaleDirective::ZhCn));
}

#[test]
fn clearing_locale_does_not_clear_theme() {
    let state = Directive { theme_dir: Some(PathBuf::from("orange")), locale: Some(LocaleDirective::ZhCn) };
    assert_eq!(state.with_locale(None).theme_dir, state.theme_dir);
}
```

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml daemon::tests -- --nocapture`

Expected: compilation fails because `Directive` is still `Option<PathBuf>`.

- [ ] **Step 3: Replace the directive alias with explicit state**

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directive {
    pub theme_dir: Option<PathBuf>,
    pub locale: Option<LocaleDirective>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleDirective { ZhCn }
```

Update the daemon to build theme and locale payloads independently, probe/apply/remove each stamp independently, and include locale stamp plus translated-node count in `DaemonStatus`. Excluded pet targets remain excluded from both payloads.

- [ ] **Step 4: Update `ThemeService` directive setters**

Replace direct `send(Some(dir))`/`send(None)` calls with helpers that clone the current directive and update only one field:

```rust
async fn set_theme_directive(&self, theme_dir: Option<PathBuf>) -> Result<(), AppError>;
async fn set_locale_directive(&self, locale: Option<LocaleDirective>) -> Result<(), AppError>;
```

- [ ] **Step 5: Run daemon and Tauri compile checks**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml daemon::tests -- --nocapture`

Run: `cargo check --manifest-path src-tauri/Cargo.toml --all-targets`

Expected: tests pass and every existing theme workflow compiles with locale state preserved.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/codex-theme-engine/src/daemon.rs src-tauri/src/app/codex_theme.rs
git commit -m "refactor: separate theme and locale directives"
```

### Task 3: Add verified official-first fallback orchestration

**Files:**
- Modify: `crates/codex-theme-engine/src/native_hot.rs`
- Modify: `src-tauri/src/app/codex_theme.rs`
- Modify: `src-tauri/src/commands.rs`

- [ ] **Step 1: Write failing result and decision tests**

```rust
#[test]
fn official_catalog_wins_without_runtime() {
    assert_eq!(choose_locale_mode(true, None).unwrap(), LocaleMode::OfficialCatalog);
}

#[test]
fn verified_fallback_requires_visible_translation() {
    assert_eq!(choose_locale_mode(false, Some(runtime_report(2))).unwrap(), LocaleMode::CompatibilityRuntime);
    assert!(choose_locale_mode(false, Some(runtime_report(0))).unwrap_err().to_string().contains("zero-translated-ui"));
}
```

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml native_hot::tests::locale_mode -- --nocapture`

Expected: compilation fails because the result types and chooser do not exist.

- [ ] **Step 3: Define the structured result contract**

```rust
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocaleMode { OfficialCatalog, CompatibilityRuntime, SystemDefault }

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexLocaleApplyResult {
    pub mode: LocaleMode,
    pub locale: Option<String>,
    pub translated_nodes: usize,
    pub catalog_entries: usize,
}
```

- [ ] **Step 4: Implement official-first orchestration**

Change `ThemeService::apply_locale_with_restart` to return `CodexLocaleApplyResult`. For `zh-CN`, restart and verify the setting, wait briefly for official `zh-CN-*.js`, and return `official_catalog` on success. On catalog timeout only, set the daemon locale directive, evaluate the payload immediately, then poll until marker, stamp, observer, catalog size, and `translatedNodes > 0` all match. CDP, setting, and runtime errors must not collapse into catalog timeout.

For system language, clear the locale directive before restart, write/read `localeOverride = null`, and return `system_default` with zero counts. The process restart removes prior DOM translations.

- [ ] **Step 5: Map stable command errors**

Map engine markers to `codex_locale_setting_verification_failed`, `codex_locale_cdp_unavailable`, `codex_locale_runtime_install_failed`, `codex_locale_zero_translated_ui`, and existing busy/unsupported errors. Do not log unmatched strings.

- [ ] **Step 6: Run Rust tests and compile checks**

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets`

Run: `cargo check --manifest-path src-tauri/Cargo.toml --all-targets`

Expected: all locale decisions pass without launching the real Codex app.

- [ ] **Step 7: Commit Task 3**

```bash
git add crates/codex-theme-engine/src/native_hot.rs src-tauri/src/app/codex_theme.rs src-tauri/src/commands.rs
git commit -m "feat: verify Codex locale fallback"
```

### Task 4: Show the verified localization mode in Toolbox

**Files:**
- Modify: `src/services/managerApi.ts`
- Modify: `src/services/managerApi.test.ts`
- Modify: `src/app/views/toolbox/UiLanguage.tsx`
- Modify: `src/app/views/toolbox/UiLanguage.test.tsx`
- Modify: `src/app/toolboxCopy.ts`

- [ ] **Step 1: Write failing API and UI tests**

Mock each structured result and assert distinct status copy:

```ts
vi.mocked(managerApi.toolboxApplyCodexLocale).mockResolvedValue({
  mode: "compatibility_runtime",
  locale: "zh-CN",
  translatedNodes: 12,
  catalogEntries: 180,
});
expect(await screen.findByText(/兼容汉化/)).toBeInTheDocument();
```

Also cover `official_catalog`, `system_default`, zero-translation error, busy state, and prevention of a second click while applying.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `npx vitest run src/services/managerApi.test.ts src/app/views/toolbox/UiLanguage.test.tsx --exclude "**/.worktrees/**"`

Expected: tests fail because the API returns `void` and UI has no mode state.

- [ ] **Step 3: Add TypeScript result types and render modes**

```ts
export interface CodexLocaleApplyResult {
  mode: "official_catalog" | "compatibility_runtime" | "system_default";
  locale: string | null;
  translatedNodes: number;
  catalogEntries: number;
}
```

Return this type from `toolboxApplyCodexLocale`, store it in `ApplyStatus`, and show precise success copy for official, compatibility, and system modes. Error copy must remain code-specific and must never show success after a rejected promise.

- [ ] **Step 4: Run Toolbox tests and verify GREEN**

Run: `npx vitest run src/services/managerApi.test.ts src/app/views/toolbox/UiLanguage.test.tsx src/app/views/Toolbox.test.tsx --exclude "**/.worktrees/**"`

Expected: all API and Toolbox localization tests pass.

- [ ] **Step 5: Commit Task 4**

```bash
git add src/services/managerApi.ts src/services/managerApi.test.ts src/app/views/toolbox/UiLanguage.tsx src/app/views/toolbox/UiLanguage.test.tsx src/app/toolboxCopy.ts
git commit -m "feat: report Codex localization mode"
```

### Task 5: Verify production embedding without touching real Codex state

**Files:**
- Create: `scripts/locale-runtime-contract.test.mjs`
- Modify: `package.json`

- [ ] **Step 1: Write a failing static contract test**

Assert that the runtime marker, protected selectors, independent locale directive, and structured modes are present in sources, and that no test references `WindowsApps`, `app.asar` mutation, or the audited reference package path.

- [ ] **Step 2: Run the contract test and verify RED**

Run: `npx vitest run scripts/locale-runtime-contract.test.mjs`

Expected: failure until the contract is registered and all source markers exist.

- [ ] **Step 3: Add the test to the normal suite and document static binary verification**

Add the test under the existing Vitest discovery pattern. Document `llvm-strings` checks for `cam-locale-v1`, `/index.html`, the current brand URL, and the absence of bundled OpenAI package files; do not run or install the EXE.

- [ ] **Step 4: Run full verification**

Run: `cargo fmt --all -- --check`

Run: `npm run check && npm run lint && npm test && npm run build`

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets`

Run: `cargo test --manifest-path src-tauri/Cargo.toml --all-targets`

Run: `git diff --check`

Expected: all commands pass and no command launches or writes the user's real Codex profile.

- [ ] **Step 5: Commit Task 5**

```bash
git add scripts/locale-runtime-contract.test.mjs package.json
git commit -m "test: cover Codex locale compatibility runtime"
```
