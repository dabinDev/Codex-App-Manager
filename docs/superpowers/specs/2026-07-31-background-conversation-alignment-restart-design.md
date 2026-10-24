# Background Conversation Alignment And Codex Restart Design

Date: 2026-07-31
Status: Approved in conversation

## 1. Objective

Make the existing Toolbox conversation-alignment workflow complete in one click
without opening Command Prompt, PowerShell, a terminal, or Codex CLI. A single
malformed rollout file must no longer block all valid conversations. When the
current user cannot write a target on Windows, the application may show the
standard UAC consent prompt and continue through a tightly scoped elevated copy
of the same native application.

After a successful alignment, the UI must tell the user that Codex needs to be
restarted and expose a force-restart action. The same restart action must also
remain available from the alignment page before or after an alignment run.

Automated tests and packaged smoke tests must use isolated synthetic Codex homes
and must never change the user's real Codex data.

## 2. Scope

This change includes:

- partial-safe scanning and execution for valid rollout metadata;
- repair of a narrowly defined missing `model_provider` field;
- separate reporting for skipped malformed rollout files;
- native background execution with no script or console process;
- a Windows UAC fallback that runs only after a permission-denied result;
- backup, rollback, verification, and elevated retry behavior;
- an always-available force-restart action in the alignment tool;
- a prominent restart action after a verified successful alignment;
- Windows and macOS desktop restart behavior;
- Rust, frontend, and packaged-smoke coverage using temporary data.

This change does not modify the reference scripts or `E:\ForkProject\orangeapi`.

## 3. Current Failure

The current scanner adds a warning for every rollout whose first line is not a
valid `session_meta` object with a non-empty string at
`/payload/model_provider`. `can_execute` is then derived from an empty warning
list. As a result, one malformed file changes a scan such as `432 / 433` into a
globally blocked operation.

Removing the warning gate alone is unsafe. Recovery generation currently sends
every rollout selected by the plan through the aligned-head builder, including
entries that have no parsed provider. The engine must first classify rollout
files and include only safe targets in mutation and recovery.

## 4. Chosen Architecture

### 4.1 Native alignment engine

The Tauri command continues to run the Rust engine on a blocking worker thread.
The application does not invoke `.cmd`, `.ps1`, `powershell.exe`, `cmd.exe`, a
shell, `sqlite3`, or `codex`.

The engine classifies every discovered rollout into one of three categories:

- `aligned`: valid `session_meta` with the active provider;
- `repairable`: valid `session_meta` whose provider differs or whose
  `model_provider` member is absent, null, or an empty string;
- `skipped`: oversized first line, invalid JSON, a first line other than
  `session_meta`, a missing/non-object payload, or a provider value with an
  unsupported JSON type.

Only `repairable` files enter the recovery manifest and mutation plan. Skipped
files remain byte-for-byte unchanged and do not prevent database rows or other
rollout files from being aligned.

### 4.2 Fatal and non-fatal findings

The scan response separates fatal blockers from skipped-file notices.

Fatal blockers include an invalid Codex home, invalid active configuration,
unsafe paths, unsupported database schema, database read failure, or another
exclusive application operation. A fatal blocker disables execution.

Malformed rollout metadata is non-fatal. The UI reports its aggregate count and
stable reason codes while keeping execution enabled when at least one database
row or repairable rollout needs work. When every discovered target is skipped,
the tool reports that there is nothing safe to align rather than reporting
success.

### 4.3 Process behavior

The installed Codex desktop application is stopped through the existing native
process logic before mutation. User-started Codex CLI processes are never
terminated.

An active CLI process does not cause an unconditional kill. The engine validates
each target again immediately before mutation. If a live process changes or
locks a target, that target produces a stable busy/stale result and the engine
uses the recovery path instead of overwriting uncertain data. This preserves
running CLI work while preventing unsafe writes.

## 5. Alignment Data Flow

Execution follows this order:

1. Acquire the existing global operation lock and alignment file lock.
2. Rebuild the plan and compare its semantic token with the confirmed scan.
3. Stop the installed Codex desktop application if it is running.
4. Rebuild the plan after process settlement and reject a stale impact set.
5. Create a private recovery directory and manifest.
6. Back up each SQLite database with the SQLite backup API.
7. Store the original and intended first line for each repairable rollout.
8. Update databases in transactions and rewrite rollout heads atomically.
9. Verify provider values, `PRAGMA quick_check`, JSON structure, and unchanged
   rollout tail bytes.
10. Return updated, skipped, and failed counts plus restart eligibility.

A no-op does not create a backup. Skipped rollout files are represented in the
report but not copied into the mutation recovery set because they are never
written.

## 6. Permission Escalation

### 6.1 Normal path

All work starts with the current user's privileges. The engine does not request
administrator access merely because the user clicked Align.

### 6.2 Windows UAC fallback

Only a native permission-denied result may request elevation. If permission is
denied before any mutation, the application can immediately offer or trigger
the UAC continuation. If permission is denied after any mutation, the engine
must first restore from the recovery manifest. When normal privileges cannot
complete that restore, the elevated continuation restores and verifies the
original state before retrying the complete alignment.

Elevation launches the same GUI executable through the Windows `runas` verb,
so Windows displays UAC but no console window. The elevated process accepts a
private internal mode, not arbitrary commands. Its request contains:

- a random one-time nonce;
- the canonical Codex home already approved by the parent process;
- the operation and recovery identifiers;
- the confirmed semantic scan token;
- a private result-file path under application-owned state.

The helper rejects unknown operations, reused or expired nonces, paths outside
the canonical Codex home and recovery root, changed scan tokens, unsafe links,
and result paths outside the private operation directory. It runs the existing
alignment engine, writes a structured result, and exits. The main window waits
asynchronously and remains responsive.

Cancellation of UAC returns a dedicated `elevation_cancelled` outcome and does
not turn a restored operation into an error requiring manual recovery.

### 6.3 macOS behavior

Codex metadata normally belongs to the signed-in user, so macOS uses the normal
native path without a shell or terminal. A permission failure is reported with
the affected category and leaves the verified recovery available. Adding a
privileged macOS helper is outside this change because it requires a separately
installed and signed helper service.

## 7. Recovery And Verification

No target is modified before its recoverable preimage exists. The existing
recovery manifest remains the source of truth and is extended only as needed to
record the new repairable/skipped classification and permission-retry state.

On any non-permission mutation error, the engine restores every artifact changed
by the current operation and verifies the restoration. On a permission error,
the same requirement applies before elevated retry. The final outcomes remain
explicit:

- `committed`: all eligible targets were updated and verified;
- `no_changes`: all valid metadata already matched;
- `stale_plan`: relevant data changed before mutation;
- `restored`: a failure occurred and rollback was verified;
- `recovery_required`: automatic rollback could not be verified.

`committed` can coexist with a non-zero skipped count because skipped malformed
files were outside the safe mutation set. The result copy must state both facts
instead of implying that malformed files were repaired.

## 8. Frontend Design

The existing Conversation Alignment detail page remains the single focused
surface. Its metrics add a skipped-session count without exposing conversation
content. Non-fatal skipped findings use a warning banner but do not disable the
primary Align action.

While work is running, the page shows one progress state and disables duplicate
actions. If Windows elevation is required, the page changes to a waiting-for-UAC
state. UAC cancellation, permission failure, rollback, and recovery-required
states each receive distinct localized copy.

After a verified `committed` result, the primary result action becomes **Restart
Codex** and the page states that restarting is required before the aligned
conversations appear. The same page also contains a persistent **Force Restart
Codex** command in its normal action area, available independently of alignment
success. Both commands require a confirmation when Codex is currently running.

Buttons use the existing icon system, sizing, Acid Orange tokens, and compact
layout behavior. No new nested cards or separate top-level navigation entry is
introduced.

## 9. Force-Restart Contract

Force Restart uses the existing `restart_codex_plain` native implementation and
the application's operation lock. It closes only processes classified as the
installed Codex desktop application, waits for exit, force-terminates those
desktop processes after the existing timeout when required, and then launches
the installed application again.

It never terminates a process classified as Codex CLI. Failure to identify an
installed desktop executable returns a structured error. Restart does not read
or modify conversation metadata and does not require administrator privileges
unless the operating system itself denies control of the installed desktop
process.

The post-alignment restart permit remains tied to a verified result. The
persistent manual force-restart command uses its own explicit confirmation and
does not forge an alignment permit.

## 10. Privacy And Logging

The workflow remains local and makes no network requests. Frontend responses
and logs contain aggregate counts, stable error codes, operation identifiers,
and recovery paths only. They do not contain conversation text, API keys, full
rollout lines, or provider credentials.

The helper request contains no conversation content and is deleted after the
parent consumes the result. Recovery data retains the same private permissions
as the existing alignment recovery directory.

## 11. Testing

Rust tests use injected temporary Codex homes and synthetic databases. Coverage
includes:

- one malformed rollout among hundreds of repairable files;
- missing, null, empty, wrong-type, and already aligned providers;
- invalid JSON, wrong event type, missing payload, BOM, and line-ending cases;
- skipped files remaining byte-for-byte unchanged;
- scan tokens including repairable and skipped classifications;
- backup and verified rollback after an injected mid-operation denial;
- elevated request validation, nonce replay rejection, unsafe paths, and UAC
  cancellation through mocked platform boundaries;
- desktop/CLI process classification and restart behavior;
- no-op, stale, committed-with-skips, restored, and recovery-required reports.

Frontend tests cover enabled execution with non-fatal skips, fatal blockers,
waiting for UAC, cancellation, result counts, required-restart copy, the
persistent force-restart action, confirmation, duplicate-click prevention, and
localized strings.

Packaged smoke tests point the application at a validated temporary Codex home.
They verify the background flow without invoking UAC against real system files.
No automated command may resolve or write `C:\Users\dabin\.codex` or the real
macOS Codex home.

## 12. Acceptance Criteria

- Clicking Align never opens a terminal, shell, script host, or Codex CLI.
- One malformed rollout does not block valid databases or rollout files.
- Missing providers are repaired only in structurally valid `session_meta`.
- Malformed files remain unchanged and are reported separately.
- Windows requests UAC only after a real permission-denied result.
- A mid-operation denial cannot leave an unverified partial alignment.
- The elevated helper cannot execute arbitrary commands or escape approved
  Codex/recovery directories.
- User-started Codex CLI processes are never force-terminated.
- A verified success clearly requires restart and provides the action.
- Force Restart is available from the alignment page at any time and affects
  only the installed Codex desktop application.
- Windows and macOS supported builds compile and pass their platform tests.
- Automated verification never changes the user's real Codex data.

## 13. Non-Goals

- Repairing arbitrary JSONL corruption or reconstructing missing conversations.
- Editing conversation content, titles, timestamps, or messages.
- Killing user-started Codex CLI tasks.
- Running administrator access unconditionally.
- Shipping or invoking the provided CMD/PowerShell scripts.
- Installing a privileged macOS helper service.
- Modifying `E:\ForkProject\orangeapi`.
