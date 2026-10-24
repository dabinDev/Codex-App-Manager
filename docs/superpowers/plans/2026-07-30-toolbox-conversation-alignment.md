# Toolbox Conversation Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an extensible Toolbox tab whose first native Windows/macOS tool safely aligns Codex database and rollout provider metadata with backup, verification, undo, and restart controls.

**Architecture:** A typed React tool registry renders the Toolbox shell while a focused Rust module owns all local data access. Scan and execute are separate IPC commands joined by a semantic scan token; mutation is serialized, backed up before writes, verified afterward, and restored on failure.

**Tech Stack:** React 19, TypeScript 6, Tauri v2, Rust 2021, `rusqlite` bundled SQLite/backup API, `toml`, `sysinfo`, Serde, fs4, SHA-256, Vitest/Testing Library.

---

## File Map

- Create `src-tauri/src/app/conversation_alignment.rs`: native scan, backup, mutation, verification, restore, and unit tests.
- Modify `src-tauri/src/app/mod.rs`: export the alignment module.
- Modify `src-tauri/src/app/paths.rs`: safe `CODEX_HOME` and smoke-fixture resolution.
- Modify `src-tauri/src/app/codex_theme.rs`: expose installed-desktop path and safe restart helpers within the crate.
- Modify `src-tauri/src/app/oplock.rs`: add the alignment operation kind.
- Modify `src-tauri/src/commands.rs`: add scan, execute, restore, open-backup, and restart commands.
- Modify `src-tauri/src/lib.rs`: register the new commands.
- Modify `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`: add native dependencies.
- Create `src/app/views/toolbox/registry.ts`: typed tool registry.
- Create `src/app/views/toolbox/ConversationAlignment.tsx`: alignment state machine and UI.
- Create `src/app/views/toolbox/ConversationAlignment.test.tsx`: UI workflow tests.
- Create `src/app/views/Toolbox.tsx`: list/detail shell.
- Create `src/app/views/Toolbox.test.tsx`: registry and navigation tests.
- Modify `src/app/App.tsx`, `src/app/Rail.tsx`, and tests: route and rail entry.
- Modify `src/shared/types.ts` and `src/services/managerApi.ts`: IPC contracts.
- Modify `src/app/i18n.tsx`: localized Toolbox copy for every supported language.
- Modify `src/app/styles.css`: Toolbox layout and state styles.
- Modify `docs/privacy.md`: disclose user-initiated local metadata repair.

### Task 1: Establish safe Codex-home and native dependencies

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`
- Modify: `src-tauri/src/app/paths.rs`

- [ ] **Step 1: Write failing path-resolution tests**

Add pure tests around an injected environment and home value:

```rust
#[test]
fn codex_home_prefers_a_valid_absolute_override() {
    let fallback = PathBuf::from("/users/test/.codex");
    let selected = select_codex_home(Some(OsString::from("/tmp/custom-codex")), Some(fallback));
    assert_eq!(selected, Some(PathBuf::from("/tmp/custom-codex")));
}

#[test]
fn invalid_smoke_codex_home_never_falls_back_to_production() {
    assert_eq!(select_codex_home_for_smoke(SmokeDataDir::Invalid, Some(PathBuf::from("real"))), None);
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::paths::tests::codex_home
```

Expected: compilation fails because the selection helpers do not exist.

- [ ] **Step 3: Implement path selection and add dependencies**

Add these dependencies:

```toml
rusqlite = { version = "0.38", features = ["backup", "bundled"] }
sysinfo = "0.37"
toml = "0.9"
```

Make `codex_home_dir()` honor a non-empty absolute `CODEX_HOME`, and add a
validated `CAM_PACKAGED_SMOKE_CODEX_HOME` path that must be a real non-symlink
directory directly below the validated smoke data directory. Invalid smoke
markers return `None`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::paths::tests
```

Expected: all path tests pass and Cargo resolves bundled SQLite on Windows.

- [ ] **Step 5: Commit the path foundation**

```powershell
git add -- src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/app/paths.rs
git commit -m "feat: add safe Codex home resolution"
```

### Task 2: Implement read-only conversation preflight

**Files:**
- Create: `src-tauri/src/app/conversation_alignment.rs`
- Modify: `src-tauri/src/app/mod.rs`

- [ ] **Step 1: Write scanner tests with synthetic SQLite and JSONL fixtures**

Define the public scan contract and fixture helpers in the new module:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentScan {
    pub scan_token: String,
    pub active_provider: String,
    pub database_count: usize,
    pub thread_count: u64,
    pub mismatched_threads: u64,
    pub rollout_count: usize,
    pub mismatched_rollouts: usize,
    pub desktop_running: bool,
    pub cli_running: bool,
    pub can_execute: bool,
    pub warnings: Vec<String>,
}
```

Cover top-level TOML parsing, root plus `sqlite/` discovery, canonical de-dup,
schema rejection, already-aligned data, mismatches, archived sessions, CRLF,
UTF-8 BOM, malformed first-line JSON, and a stable token independent of file
enumeration order.

- [ ] **Step 2: Run scanner tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests::scan
```

Expected: compilation fails because the scanner is not implemented.

- [ ] **Step 3: Implement structural parsing and read-only scan**

Implement focused internal units:

```rust
fn active_provider(config: &[u8]) -> Result<String, AlignmentError>;
fn discover_databases(home: &Path) -> Result<Vec<PathBuf>, AlignmentError>;
fn discover_rollouts(home: &Path) -> Result<Vec<PathBuf>, AlignmentError>;
fn scan_database(path: &Path, provider: &str) -> Result<DatabaseImpact, AlignmentError>;
fn scan_rollout(path: &Path, provider: &str) -> Result<RolloutImpact, AlignmentError>;
fn semantic_token(plan: &AlignmentPlan) -> String;
```

Open databases read-only, set a 15-second busy timeout, validate the column via
`pragma_table_info`, and count with parameterized queries. Read only the first
JSONL line for scan; update only `payload.model_provider` when
`type == "session_meta"`.

- [ ] **Step 4: Run scanner tests and verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests::scan
```

Expected: all scan tests pass without reading the production Codex home.

- [ ] **Step 5: Commit preflight support**

```powershell
git add -- src-tauri/src/app/conversation_alignment.rs src-tauri/src/app/mod.rs
git commit -m "feat: scan Codex conversation provider metadata"
```

### Task 3: Implement backup, alignment, verification, and undo

**Files:**
- Modify: `src-tauri/src/app/conversation_alignment.rs`

- [ ] **Step 1: Add failing transactional tests**

Add tests for no-op, committed update, stale token, backup-before-mutation,
SQLite `quick_check`, rollout-tail preservation, injected failure rollback,
rollback verification failure, and undo after a committed run:

```rust
assert_eq!(report.outcome, AlignmentOutcome::Committed);
assert!(report.write_verified);
assert!(report.backup_dir.as_ref().is_some_and(|path| path.exists()));
assert_eq!(tail_after, tail_before);

let restored = restore_alignment_at(&paths, &report.recovery_id).unwrap();
assert_eq!(restored.outcome, AlignmentOutcome::Restored);
assert!(restored.rollback_verified);
```

- [ ] **Step 2: Run mutation tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests::alignment
```

Expected: compilation fails because execute and restore do not exist.

- [ ] **Step 3: Implement recovery manifest and backups**

Use versioned Serde types:

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryManifest {
    version: u32,
    recovery_id: String,
    provider: String,
    databases: Vec<DatabaseBackup>,
    rollouts: Vec<RolloutBackup>,
}
```

Create a private `recovery-backups/provider-align-<time>-<uuid>` directory,
back up databases through `rusqlite::backup::Backup`, and persist original
rollout first lines plus hashes before mutation. Sync backup files and the
manifest before the first write.

- [ ] **Step 4: Implement execute and verified rollback**

Expose:

```rust
pub fn execute_alignment_at(
    paths: &AlignmentPaths,
    scan_token: &str,
    process: &dyn ProcessGate,
) -> Result<AlignmentReport, AlignmentError>;

pub fn restore_alignment_at(
    paths: &AlignmentPaths,
    recovery_id: &str,
) -> Result<AlignmentReport, AlignmentError>;
```

Use transactions and bound parameters for SQLite. Rewrite JSONL through a
same-directory unique temporary file, copy the untouched tail as bytes, sync,
and atomically replace. On failure restore every mutated artifact and verify
the original provider counts/heads. Return `recovery_required` when any restore
verification fails.

- [ ] **Step 5: Run mutation tests and verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests
```

Expected: all scan, execute, rollback, and restore tests pass.

- [ ] **Step 6: Commit the native transaction engine**

```powershell
git add -- src-tauri/src/app/conversation_alignment.rs
git commit -m "feat: align and restore Codex conversation metadata"
```

### Task 4: Wire process safety and Tauri commands

**Files:**
- Modify: `src-tauri/src/app/codex_theme.rs`
- Modify: `src-tauri/src/app/oplock.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing process and command-plan tests**

Cover executable-path classification and restart gating:

```rust
assert_eq!(classify_codex_process(&desktop_exe, &installed_root), ProcessClass::Desktop);
assert_eq!(classify_codex_process(&cli_exe, &installed_root), ProcessClass::Cli);
assert!(!restart_allowed(AlignmentOutcome::RecoveryRequired, true, false));
assert!(restart_allowed(AlignmentOutcome::Committed, true, true));
```

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml conversation_alignment commands::tests::alignment
```

Expected: compilation fails because process inventory and commands are absent.

- [ ] **Step 3: Add operation and IPC commands**

Add `OperationKind::ConversationAlignment` and commands with these contracts:

```rust
#[tauri::command]
pub async fn toolbox_alignment_scan() -> Result<AlignmentScan, CommandError>;

#[tauri::command]
pub async fn toolbox_alignment_execute(
    state: State<'_, ManagerState>,
    scan_token: String,
) -> Result<AlignmentReport, CommandError>;

#[tauri::command]
pub async fn toolbox_alignment_restore(
    state: State<'_, ManagerState>,
    recovery_id: String,
) -> Result<AlignmentReport, CommandError>;
```

Also register open-backup and restart commands. Run blocking filesystem/SQLite
work through `spawn_blocking`; never hold a Tokio mutex across it. Reuse the
existing installed-app graceful stop and themed/plain restart decision.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml conversation_alignment commands::tests::alignment
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: tests and compile checks pass on Windows host code.

- [ ] **Step 5: Commit native command wiring**

```powershell
git add -- src-tauri/src/app/codex_theme.rs src-tauri/src/app/oplock.rs src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: expose conversation alignment commands"
```

### Task 5: Add Toolbox routing and typed frontend contracts

**Files:**
- Create: `src/app/views/toolbox/registry.ts`
- Create: `src/app/views/Toolbox.tsx`
- Create: `src/app/views/Toolbox.test.tsx`
- Modify: `src/app/App.tsx`
- Modify: `src/app/App.test.tsx`
- Modify: `src/app/Rail.tsx`
- Modify: `src/app/Rail.test.tsx`
- Modify: `src/shared/types.ts`
- Modify: `src/services/managerApi.ts`
- Modify: `src/services/managerApi.test.ts`
- Modify: `src/app/i18n.tsx`

- [ ] **Step 1: Write failing navigation and API-contract tests**

Require rail order `home, config, themes, toolbox, settings`, active Toolbox
state, list rendering, compact back navigation, and exact invoke payload keys:

```tsx
expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(
  expect.arrayContaining(["主页", "API 配置", "皮肤", "工具箱", "设置"]),
);
fireEvent.click(screen.getByRole("button", { name: "工具箱" }));
expect(screen.getByRole("heading", { name: "工具箱" })).toBeInTheDocument();
```

- [ ] **Step 2: Run focused frontend tests and verify RED**

Run:

```powershell
$env:PATH='F:\Git\bin;'+$env:PATH
npx vitest run src/app/App.test.tsx src/app/Rail.test.tsx src/app/views/Toolbox.test.tsx src/services/managerApi.test.ts
```

Expected: missing route, component, types, and API methods fail.

- [ ] **Step 3: Implement route, registry, and API types**

Add `toolbox` to `View` and `RailSection`. Define the registry without importing
native APIs:

```ts
export const TOOLS = [
  { id: "conversation-alignment", icon: "messages", titleKey: "toolbox.alignment.title" },
] as const satisfies readonly ToolDefinition[];
```

Add camelCase scan/report types matching Rust, then methods for scan, execute,
restore, open backup, and restart in `managerApi`.

- [ ] **Step 4: Add every locale key and run i18n tests**

Add Toolbox navigation, state, warning, confirmation, result, recovery, undo,
and restart copy to every language block in `i18n.tsx`.

Run:

```powershell
npx vitest run src/app/i18n.test.tsx src/app/App.test.tsx src/app/Rail.test.tsx src/app/views/Toolbox.test.tsx src/services/managerApi.test.ts
```

Expected: all focused tests pass and no locale key is missing.

- [ ] **Step 5: Commit the Toolbox shell**

```powershell
git add -- src/app/App.tsx src/app/App.test.tsx src/app/Rail.tsx src/app/Rail.test.tsx src/app/views/Toolbox.tsx src/app/views/Toolbox.test.tsx src/app/views/toolbox/registry.ts src/shared/types.ts src/services/managerApi.ts src/services/managerApi.test.ts src/app/i18n.tsx
git commit -m "feat: add extensible Toolbox navigation"
```

### Task 6: Implement the Conversation Alignment UI state machine

**Files:**
- Create: `src/app/views/toolbox/ConversationAlignment.tsx`
- Create: `src/app/views/toolbox/ConversationAlignment.test.tsx`
- Modify: `src/app/views/Toolbox.tsx`
- Modify: `src/app/styles.css`

- [ ] **Step 1: Write failing workflow tests**

Mock `managerApi` and cover automatic scan, exact impact display, disabled
execute for CLI, confirmation, stale rescan, committed actions, verified
rollback, recovery-required restart suppression, undo, and compact-width text:

```tsx
expect(await screen.findByText("12 个会话需要对齐")).toBeInTheDocument();
fireEvent.click(screen.getByRole("button", { name: "开始对齐" }));
expect(screen.getByRole("dialog", { name: "确认对齐" })).toBeInTheDocument();
expect(screen.queryByRole("button", { name: "重新打开 Codex" })).not.toBeInTheDocument();
```

- [ ] **Step 2: Run the component test and verify RED**

Run:

```powershell
npx vitest run src/app/views/toolbox/ConversationAlignment.test.tsx
```

Expected: test fails because the component is absent.

- [ ] **Step 3: Implement explicit finite UI states**

Use a discriminated union instead of interdependent booleans:

```ts
type AlignmentUiState =
  | { kind: "scanning" }
  | { kind: "ready"; scan: AlignmentScan }
  | { kind: "running"; scan: AlignmentScan }
  | { kind: "result"; scan: AlignmentScan; report: AlignmentReport }
  | { kind: "error"; code: string; previous?: AlignmentScan };
```

Reuse `NavBar`, `Sheet`, `StatusBanner`, and the app's focus/reduced-motion
contracts. Keep the confirmation primary action explicit. Gate restart and undo
from report fields rather than inferred copy.

- [ ] **Step 4: Add responsive styles and verify GREEN**

Add unframed divided tool rows, stable metric cells, wrapping recovery paths,
and a single-column compact layout. Then run:

```powershell
npx vitest run src/app/views/toolbox/ConversationAlignment.test.tsx src/app/views/Toolbox.test.tsx src/app/App.test.tsx
npm run check
```

Expected: workflow tests and TypeScript pass.

- [ ] **Step 5: Commit the alignment UI**

```powershell
git add -- src/app/views/toolbox/ConversationAlignment.tsx src/app/views/toolbox/ConversationAlignment.test.tsx src/app/views/Toolbox.tsx src/app/styles.css
git commit -m "feat: add conversation alignment workflow"
```

### Task 7: Update privacy and run complete verification

**Files:**
- Modify: `docs/privacy.md`
- Verify: all implementation files

- [ ] **Step 1: Update the local-data disclosure**

State in English and Chinese that the app does not upload conversations, and
that a user-initiated Toolbox repair reads/modifies only provider metadata with
local backups. Keep credentials and content excluded from logs.

- [ ] **Step 2: Run all frontend gates**

```powershell
$env:PATH='F:\Git\bin;'+$env:PATH
npm run check
npm run lint
npx vitest run --maxWorkers=1 --testTimeout=15000
npm run build
```

Expected: zero errors and all test files pass.

- [ ] **Step 3: Run all Rust gates**

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-win-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets
```

Expected: all commands exit zero on the local host; required CI repeats Rust on
both Windows and macOS.

- [ ] **Step 4: Verify no real state or reference repository changed**

Hash the real `config.toml`, `auth.json`, discovered state databases, and source
scripts before and after verification. Run packaged smoke only with a validated
temporary Codex home. Compare `git -C E:\ForkProject\orangeapi status --short`
to its recorded baseline and require no new changes.

- [ ] **Step 5: Commit docs and verification adjustments**

```powershell
git add -- docs/privacy.md
git commit -m "docs: disclose local conversation metadata repair"
```
