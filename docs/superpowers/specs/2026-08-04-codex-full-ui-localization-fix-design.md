# Codex Full UI Localization Fix Design

## Goal

Fix the Toolbox language feature so selecting Simplified Chinese changes both
Codex's native Electron menus and its React renderer UI. The existing
`--lang=zh-CN` launch argument remains responsible for the Chromium/native-menu
locale, while Codex's official `localeOverride` setting becomes the authoritative
renderer locale.

The fix must not patch `app.asar`, modify the installed Windows Store package,
or depend on binaries from `codex-plusplus-release`. Automated verification must
not write to the user's real Codex installation or settings.

## Root Cause

The current implementation assumes Codex derives its complete UI language from
Chromium's `navigator.language`. In current Codex builds, `--lang=zh-CN` reaches
the Electron and Chromium processes, but the React application resolves its
catalog through its own settings layer. That layer exposes the official
`localeOverride` setting and may otherwise retain an IDE/account-selected locale.

Consequently, restarting with only `--lang=zh-CN` can translate the top menu while
leaving the main application in English. Treating a successful restart as proof
of localization creates the false-success state reported by the user.

## Architecture

The existing native theme hot-apply module already discovers Codex's
`get-setting` and `set-setting` APIs through a loopback-only CDP session. Extract
its setting access into a small reusable operation that accepts a setting key and
JSON value, writes the value, reads it back, and compares the result.

Add a dedicated Tauri command for applying the Codex UI locale. It will:

1. validate the requested locale (`zh-CN` or system default);
2. close Codex and relaunch it with CDP bound to `127.0.0.1`;
3. retain `--lang=zh-CN` for Simplified Chinese native menus;
4. write `localeOverride = "zh-CN"` for Simplified Chinese, or `null` for system
   default;
5. read the setting back and reject any mismatch;
6. return success only after persistence is verified.

The frontend continues to persist `codexLocale` as the manager's requested
language. The language tool calls the dedicated apply command instead of treating
the generic force-restart command as localization. Existing theme and
conversation-alignment restart behavior remains unchanged.

## State And Error Handling

The operation is serialized with the existing Codex mutation lock so language,
theme, and conversation-alignment actions cannot race while stopping or restarting
Codex. Unsupported platforms, missing Codex installations, CDP startup failures,
setting-adapter discovery failures, write failures, and read-back mismatches are
reported as distinct failures through the existing application error channel.

The UI keeps its current selection but does not show a completed state when apply
or verification fails. It presents the returned diagnostic and allows retry.
Selecting system default explicitly clears `localeOverride`; merely omitting
`--lang` is not considered sufficient restoration.

The first implementation uses Codex's shipped localization catalogs and official
settings API. If a future Codex build disables its internationalization catalog
behind an account feature gate, the verified write may succeed while renderer
language verification fails. That case must be reported rather than silently
falling back to invasive package patching. A managed-copy loader similar to
`codex-plusplus-release` is outside this fix and would require a separate design.

## Testing

Implementation follows red-green TDD without launching or modifying the user's
real Codex environment:

1. Rust unit tests cover setting write/read-back success, mismatches, `zh-CN`, and
   clearing to `null` through injected CDP responses.
2. Command-level tests cover locale validation, loopback-only CDP launch options,
   persistence verification, and error propagation.
3. Frontend tests prove the language tool invokes the locale-specific command,
   does not report success on failure, and maps system default to `null`.
4. Regression tests preserve existing theme and generic force-restart behavior.
5. Final verification runs the focused Rust and frontend suites, repository
   checks, production build, `codex review --uncommitted`, and `git diff --check`.

No automated test installs, launches, or rewrites the real installed Codex app.
