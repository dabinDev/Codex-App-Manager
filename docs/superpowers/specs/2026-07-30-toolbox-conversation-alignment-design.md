# Toolbox And Conversation Alignment Design

Date: 2026-07-30
Status: Approved in conversation

## 1. Objective

Add an extensible first-class **Toolbox** section to Codex App Manager. Its
first tool ports the user's `对话回复对齐.ps1` workflow into the packaged
application: align Codex conversation metadata to the top-level
`model_provider` in the active `config.toml`, so history remains visible after
switching providers.

The implementation must work on supported Windows and macOS builds without an
external PowerShell, shell, or `sqlite3` installation. It must also refresh the
application's full color and icon system using `E:\ForkProject\orangeapi` as a
read-only visual reference.

## 2. Scope

This change includes:

- a new navigation section named Toolbox;
- a reusable frontend tool registry and toolbox list/detail shell;
- a native Rust conversation-alignment engine;
- read-only preflight, explicit confirmation, backup, mutation, verification,
  rollback, restore, and Codex restart flows;
- Windows and macOS process/path behavior;
- local-only privacy and structured error reporting;
- a global blue/neutral color-token refresh;
- a consistent outline icon system exposed through the existing `Icon` API;
- frontend, Rust, packaged-smoke, and visual regression coverage.

The source script and `orangeapi` repository remain unchanged.

## 3. Architecture

### 3.1 Toolbox shell

`Toolbox.tsx` renders tools from a typed registry. A tool entry owns a stable
identifier, localized label, icon, availability, and detail component. The
first entry is `conversation-alignment`; adding a later tool does not require a
new top-level branch in `App.tsx`.

The rail exposes Toolbox between Themes and Settings. Expanded mode uses the
rail as usual. Compact mode keeps the detail view reachable through its own
navigation bar and back action.

### 3.2 Native alignment engine

`conversation_alignment.rs` owns discovery, scanning, backup, mutation,
verification, and restoration. The frontend never receives conversation text
or direct file contents. Tauri commands return counts, state flags, paths to
the created recovery directory, and stable error codes.

SQLite support is compiled into the application through `rusqlite` with its
bundled SQLite and backup APIs. TOML is parsed structurally. JSONL session
metadata is parsed with `serde_json`; no regular-expression replacement is
used for structured values.

### 3.3 Operation isolation

Conversation alignment uses the application's cross-process operation lock and
a tool-specific file lock. It cannot overlap install, update, uninstall,
provider-write, theme, or another alignment operation.

The engine distinguishes the installed desktop Codex process from CLI Codex
processes by executable path. It may gracefully stop the installed desktop
application after confirmation. It never terminates a CLI process. An active or
unclassifiable CLI Codex process blocks mutation and leaves scan available.

## 4. Path And Discovery Contract

Codex home resolution follows this order:

1. a valid absolute `CODEX_HOME` environment value;
2. the platform user home joined with `.codex`.

Packaged smoke can override this path only with a private, validated child of
the existing smoke data directory. Invalid smoke markers fail closed and never
fall back to the real home directory.

The scanner looks for `state_*.sqlite` files in the Codex home root and its
`sqlite` child, canonicalizes them, and de-duplicates the same physical path.
It scans `rollout-*.jsonl` recursively below `sessions` and
`archived_sessions`. Unsafe symlink destinations, non-files, and paths outside
the canonical Codex home are rejected.

## 5. Preflight Contract

Opening the alignment tool starts a read-only scan. The response includes:

- active provider;
- database count;
- total and mismatched thread counts;
- rollout count and mismatched rollout count;
- desktop and CLI process state;
- whether execution is currently allowed;
- stable warnings and a scan token.

The scan validates the `threads.model_provider` schema before counting. A scan
token binds the active provider, canonical artifact set, relevant metadata,
and semantic counts. Execute recomputes this state. A changed provider, file
set, or impact count returns `stale_plan` without mutation.

## 6. Mutation And Recovery Contract

Execution follows this order:

1. acquire the global and alignment locks;
2. reject active CLI processes;
3. validate the scan token;
4. gracefully stop the installed desktop Codex when running;
5. rescan after process settlement and reject semantically changed impact;
6. create a private recovery directory and manifest;
7. back up each SQLite database with the SQLite online backup API;
8. record every rollout's original first line and file identity;
9. update databases in transactions with parameterized SQL;
10. rewrite rollout first lines through same-directory temporary files;
11. verify provider counts, `PRAGMA quick_check`, first-line JSON, and unchanged
    rollout tail bytes;
12. return a structured result.

No-op scans return `no_changes` and do not create a backup.

On mutation failure the engine restores all modified artifacts and verifies the
restored state. Outcomes are:

- `committed`;
- `no_changes`;
- `stale_plan`;
- `restored`;
- `recovery_required`.

Restart is available after `committed`, and after a pre-mutation or verified
rollback result when the tool stopped Codex. Restart is unavailable while
recovery is required. A successful run exposes an explicit undo command backed
by the manifest; undo restores only the metadata changed by the tool and does
not overwrite later conversation-tail appends.

## 7. Frontend Flow

The Toolbox list uses compact divided rows rather than nested cards. Each row
shows icon, title, availability, and a chevron. Selecting Conversation
Alignment opens a focused detail screen.

The detail screen has these states:

- scanning;
- ready with impact summary;
- blocked by CLI or unsupported data;
- confirmation sheet;
- running;
- committed/no changes;
- restored;
- recovery required.

The primary action is disabled when scan state is stale, unsupported, busy, or
blocked by CLI. The confirmation sheet states the exact impact counts and that
the desktop Codex may close. Result actions include rescan, open backup folder,
undo, and restart when allowed.

All controls remain usable in the expanded `1100x720`, minimum `960x640`, and
compact `400x640` layouts. Dynamic paths wrap or truncate with a title/accessible
name and never resize fixed controls.

## 8. Visual System

The new visual system takes these cues from `orangeapi`:

- blue interaction scale centered on `#3B82F6`, with `#2563EB` hover and
  `#1D4ED8` pressed states;
- clear neutral surface hierarchy;
- restrained shadows and borders;
- consistent outline icons.

It does not copy Vue/Tailwind implementation or decorative mesh gradients.
Light mode uses neutral white/gray surfaces; dark mode uses neutral charcoal
surfaces so the application is not dominated by dark blue. Green, amber, and
red remain reserved for semantic outcomes. Cards and controls use 6-8px radii.

`lucide-react` supplies interface icons. The existing `Icon` component remains
the only view-facing icon API and maps stable application names to Lucide
components. Codex brand artwork remains unchanged.

## 9. Privacy And Logging

The tool runs locally and makes no network requests. It never emits conversation
text, API keys, complete JSONL lines, or full per-session paths to logs or
frontend events. Logs contain operation IDs, aggregate counts, outcome codes,
and redacted artifact classes only.

The privacy policy changes from an unconditional statement that the app does
not read conversations to an explicit statement that user-initiated Toolbox
repairs may read and modify local conversation metadata without uploading it.

## 10. Testing

Rust unit tests use injected temporary Codex homes and synthetic databases. They
cover provider parsing, discovery/de-duplication, schemas, no-op, stale plans,
WAL databases, backups, JSONL line endings/BOM, atomic replacement, injected
failures, verified rollback, recovery-required outcomes, undo, and process
classification. Tests never resolve the production Codex home.

Frontend tests cover registry ordering, rail selection, compact navigation,
scan/confirm/run/result states, CLI blocking, stale plans, rollback, undo,
restart gating, and every supported locale key.

Verification includes TypeScript, ESLint, Vitest, production build, Rust fmt,
Clippy, workspace tests, standalone engine tests, Windows/macOS CI, packaged
smoke with a validated temporary Codex home, and browser screenshots at desktop
and compact sizes in light and dark themes.

## 11. Acceptance Criteria

- Toolbox is visible as a first-class rail entry on Windows and macOS.
- Conversation Alignment reproduces the useful behavior of the source script
  without external runtime dependencies.
- Preflight is read-only and mutation requires explicit confirmation.
- Backups exist before mutation and every success/rollback is verified.
- CLI processes are never killed; active CLI blocks mutation.
- Restore and restart actions obey outcome safety gates.
- The Toolbox architecture accepts later tools without new shell routing.
- The complete app uses the approved blue/neutral tokens and consistent icons.
- All supported layouts, themes, languages, and platform gates pass.
- Real local Codex data is never touched by automated tests.
- `E:\ForkProject\orangeapi` and the desktop source scripts remain unchanged.

## 12. Non-Goals

- A downloadable third-party plugin or arbitrary script execution system.
- Editing conversation messages, timestamps, titles, or content.
- Automatically killing CLI sessions.
- Uploading, synchronizing, or indexing conversation content.
- Changing OrangeAPI server behavior or source code.
