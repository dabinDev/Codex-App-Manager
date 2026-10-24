# Codex Renderer Localization Fallback Design

## Goal

Make the Toolbox Simplified Chinese action localize both the native Electron
menu and the visible Codex renderer UI, even when Codex ships a Chinese catalog
but keeps its `enable_i18n` feature gate disabled for the current account.

The implementation must not modify the Microsoft Store package, patch
`app.asar`, copy code or translation assets from `codex-plusplus-release`, or
rewrite user conversation content. Selecting the system language must restore a
clean Codex process without the compatibility runtime.

## Confirmed Root Cause

Codex 26.727.6591.0 contains `webview/assets/zh-CN-*.js` and exposes the
official `localeOverride` setting. Its renderer still evaluates
`enable_i18n` with a false fallback before it creates the internationalization
provider. `--lang=zh-CN` therefore changes Chromium and the native application
menu while the React renderer can remain English.

The existing manager correctly writes and reads back `localeOverride`, then
waits for the real Chinese catalog. It reports a catalog timeout rather than a
false success, but it has no fallback that translates the renderer.

## Chosen Architecture

The official Codex localization path remains authoritative. The manager first
restarts Codex with the existing loopback-only CDP launch, writes
`localeOverride = "zh-CN"`, and waits briefly for the shipped Chinese catalog.
If that catalog loads, the operation returns `official_catalog` and injects
nothing.

If the catalog does not load, the manager injects a dedicated, idempotent
renderer localization runtime through the existing CDP channel. This runtime
is manager-owned code with an independently written Simplified Chinese catalog.
It does not use source, binaries, or translation tables from the reference
project, whose release repository does not grant a source-code license.

The compatibility runtime lives separately from the theme runtime so locale
updates do not require a theme and theme removal cannot remove localization.
The existing target daemon gains an independent locale directive and reinjects
the runtime after renderer reloads or route-created targets while the manager is
running.

## Runtime Responsibilities

The runtime exposes a versioned `window.__camLocaleRuntimeV1` API and sets a
versioned marker on `document.documentElement`. Repeated evaluation updates the
catalog and reruns translation without installing a second observer.

It translates only UI-owned text surfaces:

- leaf text nodes that exactly match a catalog entry;
- `aria-label`, `title`, and `placeholder` attributes;
- menu, dialog, settings, navigation, onboarding, error, and command labels;
- text added later through one batched `MutationObserver`.

It must never translate text inside conversation turns, generated answers,
user prompts, code blocks, diffs, terminals, editors, inputs, text areas, or
content-editable elements. Protected ancestors are checked before both text and
attribute translation. Attribute changes made by the runtime are ignored by
its own observer to prevent loops.

The catalog covers the current supported Codex sample, including signed-out,
home, composer, conversation, settings, tools, account, confirmation, update,
and error surfaces. Catalog entries use exact source strings and do not include
regular-expression replacements over arbitrary text.

## Apply And Restore Flow

For Simplified Chinese:

1. acquire the existing Codex mutation lock;
2. restart Codex with `--lang=zh-CN` and loopback-only CDP;
3. preserve and reapply an active manager theme;
4. write and read back `localeOverride = "zh-CN"`;
5. return `official_catalog` if `zh-CN-*.js` loads;
6. otherwise inject the compatibility runtime and catalog;
7. wait for its marker, active observer, and at least one translated visible UI
   node before returning `compatibility_runtime`.

For system language, the manager restarts Codex without a Chromium locale
override, writes and reads back `localeOverride = null`, clears the daemon locale
directive, and performs no runtime injection. Because the compatibility layer
is process-local, the restart removes all translated DOM state without changing
the installed Codex files.

Manager-controlled Codex restart actions must honor the persisted locale and
reapply the compatibility runtime when required. Launching Codex independently
from the Windows Start menu cannot be made persistent without replacing its
launcher; this design deliberately does not take over the system shortcut.

## Result And Error Contract

The Tauri command returns a structured result:

- `mode`: `official_catalog`, `compatibility_runtime`, or `system_default`;
- `locale`: `zh-CN` or `null`;
- `translatedNodes`: the verified visible-node count for the fallback mode;
- `catalogEntries`: the installed fallback catalog size.

The frontend reports success only for one of these verified results. A written
setting without a loaded official catalog or verified fallback runtime is not a
success. Stable errors distinguish unsupported locale, settings verification,
CDP connection, runtime installation, zero translated UI nodes, and mutation
lock contention.

Diagnostics may report counts and runtime versions. They must not record
unmatched visible strings because those strings could contain user or model
content.

## Compatibility And Maintenance

Catalog and runtime versions are independent. A Codex update does not require a
runtime change unless its DOM behavior changes; text changes require only a
catalog update. Unknown UI text remains unchanged instead of being translated
heuristically.

The Toolbox status shows whether Codex is using its official catalog or the
compatibility runtime. A compatibility audit can report the number of eligible
unmatched UI nodes locally, without persisting their contents.

## Testing

Implementation follows red-green TDD and does not launch or rewrite the user's
real Codex environment.

1. JavaScript runtime tests in jsdom cover exact text and attribute replacement,
   dynamically added nodes, idempotent reinjection, and observer batching.
2. Negative tests prove prompts, replies, code, diffs, terminals, editors,
   inputs, and content-editable elements are unchanged.
3. Rust tests cover official-catalog preference, timeout-to-fallback transition,
   structured results, zero-translation rejection, system restore, and daemon
   directive independence from themes.
4. Frontend tests cover both successful modes, system restore, error copy, busy
   handling, and the absence of false success.
5. Static packaged tests confirm the locale runtime and catalog are embedded in
   the production binary without modifying or bundling an OpenAI package.
6. Manual validation uses a disposable Codex profile or a backed-up test profile;
   it never changes the user's production Codex settings during automated tests.

## Out Of Scope

- modifying or repacking the Microsoft Store Codex application;
- forcing or persisting a Statsig feature-gate override;
- replacing the Windows Start menu Codex launcher;
- translating arbitrary conversation or repository content;
- copying the reference project's unlicensed runtime or catalog.
