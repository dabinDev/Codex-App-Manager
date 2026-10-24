# Configurable Branding PR #8 Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge PR #8 into `main` while making API credentials origin-bound, driving branding and fixed updater locations from `brand.config.json`, preserving packaged checks, and surfacing categorized updater failures.

**Architecture:** PR #8 supplies one checked-in brand configuration consumed by Vite, the Rust build script, Tauri configuration sync, and release tooling. The integration adds narrow compatibility layers at security and packaging boundaries: remembered sessions carry their API origin, updater release URLs are configuration-derived, asset validation matches Vite's recursive bundle scope, and smoke scripts read the same product name. Existing `CommandError` codes remain the frontend error contract.

**Tech Stack:** React 19, TypeScript, Vite, Vitest, Tauri v2, Rust, serde, Node.js release scripts, PowerShell, Bash, Git worktrees.

---

## File Map

- `brand.config.json`: visible branding, external sites, fixed updater manifest URLs, and release artifact base URL.
- `scripts/sync-brand-config.mjs`: validates brand configuration and writes Tauri-derived fields.
- `src/app/brand.ts`: imports configuration and resolves recursively bundled frontend assets.
- `src-tauri/build.rs` and `src-tauri/src/app/brand.rs`: expose validated build-time brand constants to Rust.
- `src-tauri/src/app/orange_session.rs`: persists and enforces API service-origin binding for remembered sessions.
- `scripts/gen-updater-manifest.mjs`: generates configured product notes and artifact URLs.
- `scripts/windows-packaged-smoke.ps1` and `scripts/macos-packaged-smoke.sh`: verify custom product names without changing stable binary identifiers.
- `src/services/managerApi.ts`, `src/app/views/About.tsx`, `src/app/i18n.tsx`: preserve categorized updater failures and render localized recovery copy.
- `scripts/brand-config.test.mjs`, `scripts/updater-manifest.test.mjs`, `scripts/packaged-branding-smoke.test.mjs`, `src/app/views/About.test.tsx`: regression coverage added by this integration.

### Task 1: Create The Integration Worktree And Reapply PR #8

**Files:**
- Worktree: `E:/ForkProject/Codex-App-Manager/.worktrees/pr8-branding-integration`
- Merge: `origin/pr/8`
- Resolve: `index.html`
- Resolve: `src-tauri/src/app/mod.rs`
- Resolve: `src/app/icons.tsx`

- [ ] **Step 1: Create an isolated integration branch from the approved design commit**

Run:

```powershell
git worktree add .worktrees/pr8-branding-integration -b codex/pr8-branding-integration main
```

Expected: the new worktree starts at `f98c38d` and does not contain another `.worktrees/` directory beneath its root.

- [ ] **Step 2: Start a topology-preserving no-commit merge**

Run inside the new worktree:

```powershell
git merge --no-commit --no-ff origin/pr/8
```

Expected: conflicts only in `index.html`, `src-tauri/src/app/mod.rs`, and `src/app/icons.tsx`.

- [ ] **Step 3: Resolve all three conflicts while preserving both feature sets**

Use the following resolved forms:

```html
<link rel="icon" type="image/png" href="/favicon.png" />
<title></title>
```

```rust
pub mod atomic_file;
pub mod alignment_elevation;
pub mod alignment_command_policy;
pub mod brand;
```

```tsx
import { BRAND_LOGO_DARK, BRAND_LOGO_LIGHT } from "./brand";
import { useTheme } from "./theme";

export function CodexMark({ className }: { className?: string }) {
  const { resolved } = useTheme();
  return (
    <img
      className={className}
      src={resolved === "dark" ? BRAND_LOGO_DARK : BRAND_LOGO_LIGHT}
      alt=""
      aria-hidden="true"
      draggable={false}
    />
  );
}
```

- [ ] **Step 4: Stage the conflict resolutions without creating the merge commit**

Run:

```powershell
git add index.html src-tauri/src/app/mod.rs src/app/icons.tsx
git diff --name-only --diff-filter=U
git diff --cached --check
```

Expected: no unresolved paths and no whitespace errors. Keep `MERGE_HEAD` present until Task 7.

### Task 2: Bind Remembered API Sessions To The Configured Origin

**Files:**
- Modify: `src-tauri/src/app/orange_session.rs`
- Test: `src-tauri/src/app/orange_session.rs`

- [ ] **Step 1: Write failing origin-mismatch and legacy-metadata tests**

Add test coverage using the existing `MemoryTokenStore`:

```rust
#[tokio::test]
async fn remembered_token_is_not_restored_for_a_different_service_origin() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-data")
        .join(format!("orange-origin-mismatch-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("secret-refresh".into()))));
    let service = OrangeSessionService::for_test(
        root.join("session.json"),
        token_store.clone(),
        SessionMetadata {
            last_email: Some("user@example.test".into()),
            remember_login: true,
            site_name: None,
            service_origin: Some("https://old.example.test".into()),
        },
    );

    let view = service.session_view(OrangeProxy::Direct).await.unwrap();

    assert!(!view.authenticated);
    assert!(!view.remembered);
    assert_eq!(token_store.load().unwrap().as_deref(), Some("secret-refresh"));
    assert!(service.state.lock().await.refresh_token.is_none());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn legacy_remembered_token_without_an_origin_is_not_restored() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-data")
        .join(format!("orange-origin-legacy-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let token_store = Arc::new(MemoryTokenStore(StdMutex::new(Some("legacy-refresh".into()))));
    let service = OrangeSessionService::for_test(
        root.join("session.json"),
        token_store.clone(),
        SessionMetadata {
            last_email: Some("legacy@example.test".into()),
            remember_login: true,
            site_name: None,
            service_origin: None,
        },
    );

    let view = service.session_view(OrangeProxy::Direct).await.unwrap();

    assert!(!view.authenticated);
    assert!(!view.remembered);
    assert_eq!(token_store.load().unwrap().as_deref(), Some("legacy-refresh"));
    assert!(service.state.lock().await.refresh_token.is_none());
    let _ = fs::remove_dir_all(root);
}
```

The token is retained in the credential store but never loaded into session state or sent to an HTTP endpoint.

- [ ] **Step 2: Run the focused Rust tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests::remembered_token_is_not_restored -- --nocapture
```

Expected: compile failure because `SessionMetadata` has no `service_origin`, or behavioral failure because restore is still attempted.

- [ ] **Step 3: Add normalized origin metadata and the restore gate**

Implement:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetadata {
    last_email: Option<String>,
    remember_login: bool,
    #[serde(default)]
    site_name: Option<String>,
    #[serde(default)]
    service_origin: Option<String>,
}

fn configured_service_origin() -> &'static str {
    crate::app::brand::SERVICE_BASE_URL
}
```

Before `token_store.load()`, compare `metadata.service_origin.as_deref()` with `Some(configured_service_origin())`. On missing or mismatched origin, persist `remember_login = false`, leave `last_email` intact, do not place the token in `SessionState`, and do not send a refresh request. On successful explicit login, set `service_origin` to the configured origin before persisting metadata.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run the two new tests plus existing restore tests:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml orange_session::tests -- --nocapture
```

Expected: all `orange_session` tests pass.

- [ ] **Step 5: Stage the origin-isolation change**

Run:

```powershell
git add src-tauri/src/app/orange_session.rs
git diff --cached --check
```

### Task 3: Make Fixed Updater And Release URLs Configuration-Driven

**Files:**
- Modify: `brand.config.json`
- Modify: `scripts/sync-brand-config.mjs`
- Modify: `scripts/gen-updater-manifest.mjs`
- Modify: `docs/branding.md`
- Create: `scripts/updater-manifest.test.mjs`

- [ ] **Step 1: Write a failing non-default branding manifest test**

Extract manifest construction into an exported pure function and add:

```js
it("uses configured product and artifact base URLs", () => {
  const manifest = buildUpdaterManifest({
    tag: "v9.8.7",
    productName: "Example Installer",
    artifactBaseUrl: "https://downloads.example.test/apps",
    signedArtifacts: [{
      platform: "windows-x86_64",
      bundle: "example-setup.exe",
      signature: "signed",
    }],
    pubDate: "2026-08-01T00:00:00.000Z",
  });
  expect(manifest.notes).toBe("Example Installer v9.8.7");
  expect(manifest.platforms["windows-x86_64"].url).toBe(
    "https://downloads.example.test/apps/v9.8.7/example-setup.exe",
  );
});
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```powershell
npx vitest run scripts/updater-manifest.test.mjs
```

Expected: FAIL because generation is hardcoded to `Wangnov/Codex-App-Manager` and `Codex App Manager`.

- [ ] **Step 3: Extend and validate the configuration contract**

Add:

```json
"updates": {
  "artifactBaseUrl": "https://github.com/Wangnov/Codex-App-Manager/releases/download"
}
```

Keep `sites.updater` as the ordered list of fixed `latest.json` endpoints. Validate `updates.artifactBaseUrl` with the same HTTPS/no-credentials/no-query/no-fragment/no-trailing-slash rules and document that changing it does not enable dynamic update discovery.

- [ ] **Step 4: Make manifest generation consume brand configuration**

Read `brand.config.json`, use `brand.productName` for `notes`, and form artifact URLs as:

```js
const artifactUrl = `${brand.updates.artifactBaseUrl}/${encodeURIComponent(tag)}/${encodeURIComponent(file)}`;
```

Keep the existing required-platform, signature, SHA-256, partial-release, and summary behavior unchanged.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```powershell
npx vitest run scripts/updater-manifest.test.mjs
npm run brand:check
```

Expected: manifest tests pass and Tauri updater endpoints remain synchronized with the fixed configuration URLs.

- [ ] **Step 6: Stage updater configuration changes**

Run:

```powershell
git add brand.config.json scripts/sync-brand-config.mjs scripts/gen-updater-manifest.mjs scripts/updater-manifest.test.mjs docs/branding.md src-tauri/tauri.conf.json
```

### Task 4: Align Frontend Icon Validation With Vite Bundling

**Files:**
- Modify: `src/app/brand.ts`
- Modify: `scripts/sync-brand-config.mjs`
- Create: `scripts/brand-config.test.mjs`
- Test: `src/app/brand.test.ts`

- [ ] **Step 1: Write failing validator tests for nested and escaped paths**

Expose a pure `validateFrontendAssetPath()` helper and test:

```js
expect(validateFrontendAssetPath("src/app/assets/acme/logo.png"))
  .toBe("src/app/assets/acme/logo.png");
expect(() => validateFrontendAssetPath("src/app/other/logo.png"))
  .toThrow(/must be below src\/app\/assets/);
expect(() => validateFrontendAssetPath("src/app/assets/../secret.png"))
  .toThrow(/must be below src\/app\/assets/);
```

- [ ] **Step 2: Run the focused validator test and verify RED**

Run:

```powershell
npx vitest run scripts/brand-config.test.mjs
```

Expected: FAIL because the validator currently accepts any existing repository path and is not importable as a pure helper.

- [ ] **Step 3: Implement one recursive scope in validator and runtime**

Use normalized forward-slash paths, reject traversal and paths outside `src/app/assets/`, verify the file exists, and change Vite loading to:

```ts
const frontendAssets = import.meta.glob("./assets/**/*", {
  eager: true,
  import: "default",
  query: "?url",
}) as Record<string, string>;
```

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```powershell
npx vitest run scripts/brand-config.test.mjs src/app/brand.test.ts
npm run brand:check
```

Expected: nested assets are accepted, escaped paths are rejected, and both configured logos resolve.

- [ ] **Step 5: Stage icon validation changes**

Run:

```powershell
git add scripts/sync-brand-config.mjs scripts/brand-config.test.mjs src/app/brand.ts src/app/brand.test.ts
```

### Task 5: Make Packaged Smoke Tests Read The Brand Product Name

**Files:**
- Modify: `scripts/windows-packaged-smoke.ps1`
- Modify: `scripts/macos-packaged-smoke.sh`
- Modify: `.github/workflows/mac-packaged-smoke.yml`
- Modify: `.github/workflows/win-installer-check.yml`
- Create: `scripts/packaged-branding-smoke.test.mjs`

- [ ] **Step 1: Write failing script contract tests**

Add assertions that both scripts load `brand.config.json`, that Windows no longer defaults `ProductName` to a literal, and that macOS does not compare `CFBundleName` to a literal:

```js
expect(windows).toContain("brand.config.json");
expect(windows).not.toMatch(/\[string\]\$ProductName = "Codex App Manager"/);
expect(mac).toContain("brand.config.json");
expect(mac).not.toContain('[[ "$PRODUCT_NAME" == "Codex App Manager" ]]');
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```powershell
npx vitest run scripts/packaged-branding-smoke.test.mjs
```

Expected: FAIL on the current hardcoded defaults and comparisons.

- [ ] **Step 3: Load product name from configuration while preserving the binary name**

Windows resolves the repository root from `$PSScriptRoot`, reads JSON with `ConvertFrom-Json`, and uses `productName` only when `-ProductName` was not explicitly passed. macOS resolves the same file with a short Node command, sets `EXPECTED_PRODUCT_NAME`, and composes localized Quit labels from the localized verb plus that name. Neither script changes `codex-app-manager` process matching or internal identifiers.

- [ ] **Step 4: Pass explicit configuration paths in CI where needed**

Keep workflows deterministic by invoking scripts from repository root or passing the checked-in config path. Do not duplicate the product name in workflow YAML.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```powershell
npx vitest run scripts/packaged-branding-smoke.test.mjs scripts/packaged-alignment-smoke.test.mjs
```

Expected: both branding and existing conversation-alignment smoke contracts pass.

- [ ] **Step 6: Stage packaged-smoke changes**

Run:

```powershell
git add scripts/windows-packaged-smoke.ps1 scripts/macos-packaged-smoke.sh scripts/packaged-branding-smoke.test.mjs .github/workflows/mac-packaged-smoke.yml .github/workflows/win-installer-check.yml
```

### Task 6: Surface Categorized Manager-Updater Errors

**Files:**
- Modify: `src/services/managerApi.ts`
- Modify: `src/app/views/About.tsx`
- Test: `src/services/managerApi.test.ts`
- Create: `src/app/views/About.test.tsx`

- [ ] **Step 1: Write failing service and About-view tests**

Service test:

```ts
invokeMock.mockRejectedValueOnce({ code: "network", message: "check manager update failed" });
await expect(managerApi.checkManagerUpdate()).rejects.toMatchObject({ code: "network" });
```

View tests invoke the Check button and assert the existing localized `error.network`, `error.artifact`, and `error.signature` messages for matching `CommandError` codes. Separate cases retain `development`, `none`, and `available` results.

- [ ] **Step 2: Run focused frontend tests and verify RED**

Run:

```powershell
npx vitest run src/services/managerApi.test.ts src/app/views/About.test.tsx
```

Expected: the service test fails because `checkManagerUpdate()` currently swallows rejections into `{ kind: "unavailable" }`.

- [ ] **Step 3: Preserve command errors and use existing stable error copy**

Replace the swallowing branch with a direct invocation:

```ts
const update = await invoke<ManagerUpdateMetadata | null>("manager_check_update");
if (!update) return { kind: "none" };
return managerUpdateAvailable(update);
```

`About.tsx` already catches rejected checks and calls `userErrorMessage(cause, t)`. Preserve that path so the existing `CODE_COPY` mapping selects `error.network`, `error.artifact`, `error.signature`, `error.install`, or `error.generic`. Keep raw engine text available only through the existing diagnostic path; do not add another manager-specific error protocol.

- [ ] **Step 4: Ensure install failures remain categorized**

The install action continues to reject the original `CommandError`, and the About catch continues to call `userErrorMessage`. Add a test proving a signature or installation rejection is not replaced by the generic unavailable text.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```powershell
npx vitest run src/services/managerApi.test.ts src/app/views/About.test.tsx src/app/errorCopy.test.ts
```

Expected: all manager update state and error-copy tests pass.

- [ ] **Step 6: Stage manager-updater UI changes**

Run:

```powershell
git add src/services/managerApi.ts src/services/managerApi.test.ts src/app/views/About.tsx src/app/views/About.test.tsx
```

### Task 7: Review, Verify, Commit The Merge, And Fast-Forward Main

**Files:**
- Verify: all staged merge files
- Commit: merge commit on `codex/pr8-branding-integration`
- Fast-forward: `main`

- [ ] **Step 1: Verify the merge is structurally complete**

Run:

```powershell
git diff --name-only --diff-filter=U
git diff --cached --check
git status --short
```

Expected: no conflicts, no whitespace errors, and all intended PR plus integration files staged.

- [ ] **Step 2: Run repository review**

Run:

```powershell
codex review --uncommitted
```

Expected: no findings. If the local review command again produces no output until timeout, record that limitation and use an independent read-only review against `origin/main...HEAD` before proceeding. Fix all Critical/Important findings and repeat focused tests.

- [ ] **Step 3: Run complete frontend and script verification**

Run from the isolated worktree, which has no nested worktrees:

```powershell
$env:PATH = 'F:\Git\bin;' + $env:PATH
npm run brand:check
npm run check
npm run lint
npx vitest run --testTimeout 30000
npm run build
```

Expected: brand configuration current, zero TypeScript errors, zero ESLint warnings/errors, all Vitest files pass, and renderer entry isolation verifies.

- [ ] **Step 4: Run available Rust verification**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: formatting passes and Rust tests pass. If the host toolchain cannot link Tauri tests, run the repository's known-good Windows release compile path and report the exact host limitation rather than claiming Rust tests passed.

- [ ] **Step 5: Create the topology-preserving merge commit**

Run:

```powershell
git commit -m "feat: merge configurable branding support"
```

Expected: a two-parent merge commit whose second parent is `c77a825`.

- [ ] **Step 6: Re-review the committed range**

Run:

```powershell
codex review --base origin/main
```

Expected: no findings, or the same documented local review timeout with independent review clean.

- [ ] **Step 7: Fast-forward local main and push without force**

Run in `E:/ForkProject/Codex-App-Manager`:

```powershell
git switch main
git merge --ff-only codex/pr8-branding-integration
git push origin main
```

Expected: `main` and `origin/main` point to the same merge commit; no force push and no PR creation.

- [ ] **Step 8: Final evidence check**

Run:

```powershell
git status --short --branch
git rev-parse HEAD
git rev-parse origin/main
git show --no-patch --format="%H%n%P%n%s" HEAD
```

Expected: clean worktree, identical local/remote SHA, two merge parents, and `feat: merge configurable branding support`.
