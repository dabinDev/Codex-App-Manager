# Background Conversation Alignment And Codex Restart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align every safe Codex conversation in the background even when some rollout files are malformed, request Windows UAC only after an access-denied result, and provide a persistent native force-restart action for the Codex desktop app.

**Architecture:** The existing Rust alignment engine will classify rollouts into aligned, repairable, and skipped sets before recovery or mutation. A new elevation adapter will own the one-time request protocol and `runas` boundary, while Tauri commands coordinate normal execution, elevated retry, and restart permits. The existing React detail view will expose skipped counts, UAC progress, required-restart copy, and a separately confirmed manual force restart.

**Tech Stack:** Rust 2021, Tauri v2, `rusqlite`, Serde/JSON, SHA-256, `windows-sys`, React 19, TypeScript 6, Vitest, Testing Library.

---

## File Map

- Modify `src-tauri/src/app/conversation_alignment.rs`: rollout classification, partial-safe plans, recovery selection, permission errors, reports, and unit tests.
- Create `src-tauri/src/app/alignment_elevation.rs`: private helper request validation, one-time nonce/result files, Windows `runas` launcher, helper entry point, and unit tests.
- Modify `src-tauri/src/app/mod.rs`: export the elevation adapter.
- Modify `src-tauri/src/commands.rs`: coordinate normal alignment, elevation retry, and unrestricted confirmed force restart.
- Modify `src-tauri/src/lib.rs`: enter helper mode before Tauri single-instance startup and register the force-restart command.
- Modify `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`: add only Windows API feature flags needed for elevation and process waiting.
- Modify `src/shared/types.ts`: expose skipped counts and elevation state in IPC contracts.
- Modify `src/services/managerApi.ts` and `src/services/managerApi.test.ts`: validate new fields and invoke force restart.
- Modify `src/app/views/toolbox/ConversationAlignment.tsx` and its test: partial-safe UI, UAC state, required restart, and persistent force restart.
- Modify `src/app/toolboxCopy.ts`: localized copy for every supported language.
- Modify `src/app/i18n.test.tsx`: enforce the new keys across locales.
- Modify `src/app/styles.css`: stable fifth metric and restart action layout.
- Modify `src/app/packagedAlignmentSmoke.test.ts` and `scripts/windows-packaged-smoke.ps1`: assert isolated packaged alignment behavior without real UAC or real Codex data.

### Task 1: Classify Rollouts Without Global Blocking

**Files:**
- Modify: `src-tauri/src/app/conversation_alignment.rs`

- [ ] **Step 1: Write failing classification tests**

Add tests that create synthetic rollout first lines and require explicit classes:

```rust
#[test]
fn scan_keeps_valid_rollouts_executable_when_one_rollout_is_malformed() {
    let fixture = Fixture::new("partial-safe-scan", "model_provider = \"openai\"\n");
    fixture.rollout("valid.jsonl", session_meta(Some(json!("legacy"))));
    fixture.rollout_bytes("broken.jsonl", b"{not-json}\n");

    let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

    assert!(scan.can_execute);
    assert_eq!(scan.mismatched_rollouts, 1);
    assert_eq!(scan.skipped_rollouts, 1);
    assert_eq!(scan.skip_reasons, vec!["alignment_rollout_invalid_json"]);
}

#[test]
fn scan_repairs_missing_null_and_empty_provider_slots() {
    let fixture = Fixture::new("repairable-provider", "model_provider = \"openai\"\n");
    fixture.rollout("missing.jsonl", session_meta(None));
    fixture.rollout("null.jsonl", session_meta(Some(Value::Null)));
    fixture.rollout("empty.jsonl", session_meta(Some(json!(""))));

    let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

    assert_eq!(scan.mismatched_rollouts, 3);
    assert_eq!(scan.skipped_rollouts, 0);
}
```

Also cover wrong event type, missing/non-object payload, provider number/object/array, oversized first line, BOM, CRLF, and an already aligned string.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests::scan_
```

Expected: compilation fails because `skipped_rollouts`, `skip_reasons`, and rollout classes do not exist.

- [ ] **Step 3: Implement the minimal classification model**

Add these internal/public fields and keep the raw first line only in native memory:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RolloutClass {
    Aligned,
    Repairable,
    Skipped(&'static str),
}

pub struct AlignmentScan {
    // existing fields stay unchanged
    pub skipped_rollouts: usize,
    pub skip_reasons: Vec<String>,
}

struct RolloutImpact {
    // existing path, hashes, and raw_head stay unchanged
    class: RolloutClass,
}
```

Parse the first line structurally. Treat absent, null, empty, or mismatched string providers as repairable only when `type == "session_meta"` and `payload` is an object. Treat every unsupported structure as skipped. Derive `can_execute` from fatal blockers, not skip notices.

- [ ] **Step 4: Include classifications in the semantic token**

Hash the relative path, file identity, head hash, class discriminator, and skip reason. A malformed file changing into a valid file must invalidate an earlier scan token.

- [ ] **Step 5: Run the focused tests and verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests::scan_
```

Expected: all scan classification tests pass and no production Codex path is resolved.

- [ ] **Step 6: Commit rollout classification**

```powershell
git add -- src-tauri/src/app/conversation_alignment.rs
git commit -m "fix: allow partial conversation alignment"
```

### Task 2: Make Recovery And Execution Partial-Safe

**Files:**
- Modify: `src-tauri/src/app/conversation_alignment.rs`

- [ ] **Step 1: Write failing mutation and rollback tests**

Require valid targets to commit while malformed files remain byte-identical:

```rust
#[test]
fn alignment_commits_repairable_files_and_preserves_skipped_files() {
    let fixture = Fixture::mixed_rollouts();
    let broken_before = fs::read(fixture.home().join("sessions/broken.jsonl")).unwrap();
    let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();

    let report = execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

    assert_eq!(report.outcome, AlignmentOutcome::Committed);
    assert_eq!(report.rollouts_updated, 2);
    assert_eq!(report.rollouts_skipped, 1);
    assert_eq!(fs::read(fixture.home().join("sessions/broken.jsonl")).unwrap(), broken_before);
}

#[test]
fn missing_provider_is_inserted_without_changing_rollout_tail() {
    let fixture = Fixture::missing_provider_rollout();
    let tail_before = fixture.rollout_tail();
    let scan = scan_alignment_at(&fixture.paths, ProcessState::default()).unwrap();
    execute_alignment_at(&fixture.paths, &scan.scan_token, &TestProcessGate).unwrap();

    assert_eq!(fixture.rollout_provider().as_deref(), Some("openai"));
    assert_eq!(fixture.rollout_tail(), tail_before);
}
```

Add fault-injection coverage for permission denial before writes, after a database commit, and before a rollout replacement. Require verified rollback before an elevated retry is eligible.

- [ ] **Step 2: Run mutation tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests::alignment_
```

Expected: failing assertions because malformed rollouts still block recovery and missing provider slots cannot be written.

- [ ] **Step 3: Restrict recovery and mutation to repairable rollouts**

Replace provider-option filters with the class contract:

```rust
fn repairable_rollouts(plan: &AlignmentPlan) -> impl Iterator<Item = &RolloutImpact> {
    plan.rollouts
        .iter()
        .filter(|rollout| rollout.class == RolloutClass::Repairable)
}
```

`create_recovery`, `plan_change_count`, `apply_alignment`, and post-write verification must consume this iterator. `aligned_rollout_head` must insert `model_provider` into a valid payload object when the member is absent and replace null/empty/mismatched string values. It must reject unsupported structures even if called outside normal planning.

- [ ] **Step 4: Add permission-specific engine outcomes**

Preserve the underlying `std::io::ErrorKind::PermissionDenied` and SQLite read-only/authorization codes as:

```rust
#[derive(Debug, Error)]
pub enum AlignmentError {
    #[error("conversation metadata permission denied")]
    PermissionDenied,
    // existing variants
}

pub struct AlignmentReport {
    // existing fields stay unchanged
    pub rollouts_skipped: usize,
    pub elevation_required: bool,
}
```

Set `elevation_required` when no mutation occurred, rollback was verified, or
rollback itself failed solely with `PermissionDenied` while the private recovery
manifest remains valid. The last case selects `RestoreThenAlign`, so the helper
must restore and verify the original state before retrying. Never elevate an
unverified partial operation whose rollback failed for corruption, staleness,
or any non-permission reason.

- [ ] **Step 5: Remove unconditional CLI blocking while preserving stale checks**

Do not add `alignment_cli_running` to fatal warnings and do not call `stop_desktop` for processes classified as CLI. Continue to check file length, modified time, first-line hash, and tail hash at recovery and replacement boundaries. Return `stale_plan` or a busy error when live CLI activity changes a target.

- [ ] **Step 6: Run all engine tests and verify GREEN**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::conversation_alignment::tests
```

Expected: all classification, transaction, undo, ACL, WAL, rollback, and process-safety tests pass.

- [ ] **Step 7: Commit partial-safe execution**

```powershell
git add -- src-tauri/src/app/conversation_alignment.rs
git commit -m "fix: recover partial conversation alignment safely"
```

### Task 3: Add The Restricted Windows Elevation Protocol

**Files:**
- Create: `src-tauri/src/app/alignment_elevation.rs`
- Modify: `src-tauri/src/app/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`

- [ ] **Step 1: Write failing protocol-validation tests**

Use temporary application-state and Codex-home directories. Cover valid requests, expired/replayed nonces, unknown operations, unsafe relative paths, result paths outside the operation directory, a changed scan token, symlink escapes, and oversized JSON:

```rust
#[test]
fn helper_rejects_a_replayed_nonce() {
    let fixture = ElevationFixture::valid();
    assert!(validate_and_consume_request(&fixture.request_path).is_ok());
    assert_eq!(
        validate_and_consume_request(&fixture.request_path).unwrap_err().code(),
        "alignment_elevation_replayed"
    );
}

#[test]
fn helper_rejects_result_path_outside_private_operation_dir() {
    let fixture = ElevationFixture::with_result_path("../../outside.json");
    assert_eq!(
        validate_and_consume_request(&fixture.request_path).unwrap_err().code(),
        "alignment_elevation_invalid_request"
    );
}
```

- [ ] **Step 2: Run protocol tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::alignment_elevation::tests
```

Expected: compilation fails because the elevation module is absent.

- [ ] **Step 3: Implement private request and result contracts**

Use a single fixed operation and no arbitrary executable or arguments:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ElevationRequest {
    version: u32,
    operation: ElevationOperation,
    nonce: String,
    created_at_unix_ms: u64,
    codex_home: PathBuf,
    recovery_id: Option<String>,
    scan_token: String,
    result_file: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ElevationOperation {
    RestoreThenAlign,
    Align,
}
```

Create the request beneath a private app-state operation directory, cap request/result documents, canonicalize both approved roots, consume the nonce atomically, and delete request/result files after collection. The helper may call only alignment restore/execute functions.

Move the existing native process inventory implementation out of
`commands.rs` and expose it from `conversation_alignment.rs` as
`pub(crate) struct SystemAlignmentProcessGate`. Both normal commands and the
helper must use the same desktop/CLI classification; the helper must not use a
dummy process state.

- [ ] **Step 4: Enter helper mode before Tauri startup**

Add an early branch so the elevated second process never reaches the single-instance plugin or opens a window:

```rust
pub fn run() {
    if let Some(exit_code) = app::alignment_elevation::run_helper_from_args(std::env::args_os()) {
        std::process::exit(exit_code);
    }
    run_tauri_app();
}
```

Accept exactly the `--alignment-elevated-helper` flag followed by one absolute
request-file path and reject every other internal-helper argument shape.

- [ ] **Step 5: Implement the Windows `runas` boundary**

Under `cfg(windows)`, launch the current GUI executable with `ShellExecuteExW`, verb `runas`, the single request-path argument, `SW_HIDE`, and a process handle. Wait on a blocking worker thread, then read the structured result. Map `ERROR_CANCELLED` to `alignment_elevation_cancelled`. Under non-Windows builds, return `alignment_elevation_unsupported` without launching a shell.

Add `Win32_System_Threading` to the existing `windows-sys` feature list for
`WaitForSingleObject`; reuse the existing `Win32_Foundation`, `Win32_UI_Shell`,
and `Win32_UI_WindowsAndMessaging` features for error handling,
`ShellExecuteExW`, `SW_HIDE`, and handle cleanup. Do not add a script runtime.

- [ ] **Step 6: Run protocol tests and platform compile checks**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml app::alignment_elevation::tests
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
```

Expected: tests pass, the helper branch compiles, and no console program is referenced.

- [ ] **Step 7: Commit the elevation adapter**

```powershell
git add -- src-tauri/src/app/alignment_elevation.rs src-tauri/src/app/mod.rs src-tauri/src/lib.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat: add restricted alignment elevation helper"
```

### Task 4: Coordinate UAC Retry And Force Restart Commands

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing command-policy tests**

Extract and test pure decisions:

```rust
#[test]
fn elevation_is_allowed_only_after_verified_permission_failure() {
    assert_eq!(
        alignment_continuation(&permission_report(true, false)),
        AlignmentContinuation::ElevateAlign
    );
    assert_eq!(
        alignment_continuation(&permission_rollback_report()),
        AlignmentContinuation::ElevateRestoreThenAlign
    );
    assert_eq!(alignment_continuation(&corrupt_rollback_report()), AlignmentContinuation::StopRecoveryRequired);
}

#[test]
fn manual_force_restart_does_not_require_alignment_permit() {
    assert_eq!(restart_command_policy(false, true), RestartCommandPolicy::ConfirmedManual);
}
```

- [ ] **Step 2: Run command tests and verify RED**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commands::tests::alignment_
```

Expected: compilation fails because continuation and manual restart policies do not exist.

- [ ] **Step 3: Add elevated continuation to alignment execution**

Keep all blocking work under `spawn_blocking`. After normal execution returns a
permission report with `elevation_required == true`, let the normal worker drop
its provider/alignment cross-process permits before launching the helper. Keep
the parent Tauri operation guard active, create the restricted request, emit
`toolbox://alignment-elevation` through the `AppHandle`, invoke UAC, and replace
the normal report with the verified helper result. The helper reacquires the
native provider/alignment locks in its own process. Do not elevate other I/O,
schema, stale-plan, busy, or recovery-required errors.

Return stable codes for UAC cancellation, invalid helper result, helper timeout, and elevated alignment failure. Always release request files and operation locks on completion.

- [ ] **Step 4: Add a separate confirmed manual restart command**

Register:

```rust
#[tauri::command]
pub async fn toolbox_force_restart_codex(
    state: State<'_, ManagerState>,
    confirmed: bool,
) -> Result<(), CommandError>;
```

Reject `confirmed == false`, acquire `OperationKind::ConversationAlignment`, reuse the themed/plain restart decision and `restart_codex_plain`, and never inspect or terminate CLI-classified processes. Keep `toolbox_alignment_restart` permit-gated for post-result use.

- [ ] **Step 5: Run command and Rust checks**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commands::tests::alignment_
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: policy tests pass and both commands compile.

- [ ] **Step 6: Commit command coordination**

```powershell
git add -- src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: coordinate elevated alignment and Codex restart"
```

### Task 5: Extend Frontend Contracts And API Guards

**Files:**
- Modify: `src/shared/types.ts`
- Modify: `src/services/managerApi.ts`
- Modify: `src/services/managerApi.test.ts`

- [ ] **Step 1: Write failing API contract tests**

Require the new fields and exact manual restart payload:

```ts
await managerApi.toolboxForceRestartCodex(true);
expect(invokeMock).toHaveBeenCalledWith("toolbox_force_restart_codex", { confirmed: true });

await expect(
  guardFixture({ ...VALID_SCAN, skippedRollouts: -1 }),
).rejects.toMatchObject({ code: "contract_mismatch" });
```

Also reject non-integer skipped counts, non-string skip reasons, and non-boolean elevation flags.

- [ ] **Step 2: Run API tests and verify RED**

Run:

```powershell
npx vitest run src/services/managerApi.test.ts
```

Expected: the manual restart method and contract fields are missing.

- [ ] **Step 3: Add matching TypeScript contracts and guards**

Extend the existing interfaces:

```ts
export interface AlignmentScan {
  // existing fields stay unchanged
  skippedRollouts: number;
  skipReasons: string[];
}

export interface AlignmentReport {
  // existing fields stay unchanged
  rolloutsSkipped: number;
  elevationRequired: boolean;
}
```

Update browser-preview fixtures, runtime guards, and the new `toolboxForceRestartCodex(confirmed)` API method.
The component listens directly to the established Tauri event API for
`toolbox://alignment-elevation`; tests mock `@tauri-apps/api/event` using the
same pattern as `useDownloadProgress.test.tsx`.

- [ ] **Step 4: Run API tests and verify GREEN**

Run:

```powershell
npx vitest run src/services/managerApi.test.ts
npm run check
```

Expected: API tests and TypeScript checks pass.

- [ ] **Step 5: Commit frontend contracts**

```powershell
git add -- src/shared/types.ts src/services/managerApi.ts src/services/managerApi.test.ts
git commit -m "feat: expose alignment recovery status"
```

### Task 6: Add Partial-Safe And Restart UI States

**Files:**
- Modify: `src/app/views/toolbox/ConversationAlignment.tsx`
- Modify: `src/app/views/toolbox/ConversationAlignment.test.tsx`
- Modify: `src/app/toolboxCopy.ts`
- Modify: `src/app/i18n.test.tsx`
- Modify: `src/app/styles.css`

- [ ] **Step 1: Write failing component tests**

Cover executable-with-skips, UAC waiting/cancellation, required restart, persistent manual restart, confirmation, and duplicate-click suppression:

```tsx
it("keeps alignment enabled and reports malformed files separately", async () => {
  scanMock.mockResolvedValue({
    ...READY_SCAN,
    skippedRollouts: 1,
    skipReasons: ["alignment_rollout_invalid_json"],
    canExecute: true,
  });
  renderAlignment();

  expect(await screen.findByText("1 session file will be skipped")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Start alignment" })).toBeEnabled();
});

it("confirms and runs manual force restart independently", async () => {
  const user = userEvent.setup();
  renderAlignment();
  await user.click(await screen.findByRole("button", { name: "Force restart Codex" }));
  await user.click(screen.getByRole("button", { name: "Restart now" }));

  expect(managerApi.toolboxForceRestartCodex).toHaveBeenCalledWith(true);
});
```

Require a committed result to show both skipped count and “Restart required”.
Require recovery-required to hide the permit-gated post-result restart action.
The separately confirmed manual force-restart action remains available in every
page state, including an unresolved recovery result, because it never claims
that alignment recovery is complete.

- [ ] **Step 2: Run component tests and verify RED**

Run:

```powershell
npx vitest run src/app/views/toolbox/ConversationAlignment.test.tsx src/app/i18n.test.tsx
```

Expected: missing fields, actions, dialog, and locale keys fail.

- [ ] **Step 3: Extend the finite UI state machine**

Use distinct action states:

```ts
type ResultAction =
  | "idle"
  | "opening-backup"
  | "restoring"
  | "restarting"
  | "force-restarting";

type AlignmentUiState =
  | { kind: "scanning" }
  | { kind: "ready"; scan: AlignmentScan }
  | { kind: "confirming"; scan: AlignmentScan }
  | { kind: "executing"; scan: AlignmentScan; awaitingElevation: boolean }
  | { kind: "result"; scan: AlignmentScan; report: AlignmentReport; action: ResultAction }
  | { kind: "error"; code: string; scan?: AlignmentScan };
```

Render skipped warnings separately from fatal blockers. Compute `canStart` from `scan.canExecute` and eligible mismatch counts. After `committed`, show required-restart copy and make restart primary. Add a persistent icon-and-text force-restart button with a confirmation `Sheet`; never call the API before confirmation.
Subscribe to `toolbox://alignment-elevation` while the component is mounted.
When the event belongs to the active execution, set `awaitingElevation: true`;
always unsubscribe on unmount and ignore late events after the request settles.

- [ ] **Step 4: Add localized copy for every existing locale**

Add keys for skipped count/reasons, UAC waiting/cancelled, restart required, force restart, confirmation, progress, completion, and failure to every language block in `toolboxCopy.ts`. Keep the existing English fallback and make `i18n.test.tsx` prove every locale exposes every English key.

- [ ] **Step 5: Add stable responsive styles**

Change the metric grid to five stable cells at wide widths, two columns below the existing container breakpoint, and one column for the narrowest layout. Keep action buttons at stable height, allow labels to wrap, and use existing Acid Orange tokens and Lucide-backed icons.

- [ ] **Step 6: Run focused UI tests and verify GREEN**

Run:

```powershell
npx vitest run src/app/views/toolbox/ConversationAlignment.test.tsx src/app/i18n.test.tsx src/services/managerApi.test.ts
npm run check
```

Expected: all focused tests and TypeScript checks pass.

- [ ] **Step 7: Commit the UI workflow**

```powershell
git add -- src/app/views/toolbox/ConversationAlignment.tsx src/app/views/toolbox/ConversationAlignment.test.tsx src/app/toolboxCopy.ts src/app/i18n.test.tsx src/app/styles.css
git commit -m "feat: add background alignment and restart controls"
```

### Task 7: Isolated Smoke, Review, And Complete Verification

**Files:**
- Modify: `src/app/packagedAlignmentSmoke.test.ts`
- Modify: `scripts/windows-packaged-smoke.ps1`
- Verify: all changed implementation files

- [ ] **Step 1: Write the failing isolated smoke assertion**

Add a mixed synthetic fixture containing one repairable rollout and one invalid rollout. Require the packaged smoke result to report `committed`, one updated rollout, one skipped rollout, and unchanged invalid bytes. Record hashes for the real `config.toml`, `auth.json`, state databases, and rollout heads before launching the smoke application.

- [ ] **Step 2: Run the smoke unit test and verify RED**

Run:

```powershell
npx vitest run src/app/packagedAlignmentSmoke.test.ts
```

Expected: the old fixture/result schema has no skipped count.

- [ ] **Step 3: Update the smoke harness without invoking real UAC**

Point `CAM_PACKAGED_SMOKE_CODEX_HOME` only at the validated child fixture. Use writable fixture files so the smoke exercises native background alignment without displaying UAC. Compare the invalid rollout hash and all recorded real-data hashes after the run; fail closed if the isolation marker is missing.

- [ ] **Step 4: Run complete frontend verification**

```powershell
npm run check
npm run lint
npx vitest run --maxWorkers=1 --testTimeout=15000
npm run build
```

Expected: zero TypeScript/ESLint errors, all Vitest files pass, and the production frontend builds.

- [ ] **Step 5: Run complete Rust verification**

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --workspace
cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-win-engine/Cargo.toml --all-targets
cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets
```

Expected: every command exits zero. The Windows host covers `runas` code; CI repeats supported macOS compilation and tests.

- [ ] **Step 6: Run packaged smoke against temporary data**

Build the Windows NSIS bundle with the existing project build command, require
exactly one generated setup executable, then run:

```powershell
$setups = @(Get-ChildItem 'src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis' -Filter '*-setup.exe')
if ($setups.Count -ne 1) { throw "Expected exactly one NSIS setup, found $($setups.Count)" }
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-packaged-smoke.ps1 -InstallerPath $setups[0].FullName
```

Expected: packaged smoke succeeds, no CLI window appears, no UAC is requested for writable fixtures, and before/after hashes of the real Codex home are identical.

- [ ] **Step 7: Review until no findings remain**

```powershell
codex review --uncommitted
```

Apply valid findings, rerun the focused and complete gates affected by each change, and repeat review until it reports no actionable findings.

- [ ] **Step 8: Commit, push, and preserve the installer artifact**

```powershell
git add -- src/app/packagedAlignmentSmoke.test.ts scripts/windows-packaged-smoke.ps1
git commit -m "test: cover packaged partial conversation alignment"
git push origin "codex/橘子下载器"
```

Copy the verified unsigned NSIS installer to `E:\windows\Desktop\Codex-App-Manager-Acid-Orange-Setup.exe`, replacing only the previous generated installer artifact. Report its SHA-256 and explicitly state that the installer was not executed and the real Codex configuration was not modified.
