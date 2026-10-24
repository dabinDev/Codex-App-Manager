# Orange Downloader Brand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the Acid Orange visual identity and platform artwork to Codex App Manager without changing its workflows or writing to the user's real Codex environment.

**Architecture:** Keep the renderer behind existing CSS-token and `CodexMark` boundaries, use one canonical checked-in Acid Orange PNG, generate platform icons mechanically, and preserve the existing NSIS layout. Add static tests that make the brand source, palette, endpoint, and generated artifact requirements explicit.

**Tech Stack:** React 19, TypeScript 6, CSS custom properties, Vitest, Tauri v2 CLI, Rust, NSIS, browser visual verification.

---

### Task 1: Lock the brand contract

**Files:**
- Create: `src/app/brandContract.test.ts`
- Modify: `src/app/visualTokens.test.ts`

- [ ] Add a failing test that requires the exact Acid Orange palette, the
  `https://token.cylonai.cn` Rust constant, the canonical logo filename and
  SHA-256, and the generated platform/installer files.
- [ ] Run `npx vitest run src/app/brandContract.test.ts src/app/visualTokens.test.ts`.
- [ ] Confirm failures identify the old blue tokens and missing canonical logo.

### Task 2: Install the renderer identity

**Files:**
- Create: `src/app/assets/orange-logo.png`
- Modify: `src/app/icons.tsx`
- Modify: `src/app/icons.test.tsx`
- Modify: `src/app/styles.css`
- Modify: `src/app/staticCrashFallback.ts`
- Modify: `src/app/staticCrashFallback.test.ts`

- [ ] Copy the approved `orangeapi/frontend/public/logo.png` byte-for-byte.
- [ ] Make `CodexMark` use the canonical image in both themes and add a focused
  renderer test.
- [ ] Replace global brand, surface, text, focus, and interaction tokens with
  the approved OrangeAPI home palette while preserving semantic distinctions.
- [ ] Align the dependency-free crash surface with graphite/mist colors.
- [ ] Run focused renderer and visual-token tests until green.

### Task 3: Generate native and installer artwork

**Files:**
- Modify: `src-tauri/icons/**`
- Modify: `src-tauri/installer/header.bmp`
- Modify: `src-tauri/installer/sidebar.bmp`

- [ ] Run `npx tauri icon src/app/assets/orange-logo.png` to regenerate native
  icon outputs from the approved mark.
- [ ] Generate the 150x57 NSIS header and 164x314 NSIS sidebar in the existing
  bitmap format using graphite, mist, orange, acid, teal, and the approved mark.
- [ ] Verify image dimensions, file signatures, and source-logo contract.

### Task 4: Verify without touching the real Codex environment

**Files:**
- Modify only files required by an evidenced failure.

- [ ] Run `npm run check`, `npm run lint`, focused tests, remaining Vitest suites,
  and `npm run build`.
- [ ] Exclude only the known WSL path-invocation release test if it still fails
  in the linked Windows worktree; run that test from the normal checkout or
  record the exact environmental limitation.
- [ ] Run Rust formatting and locked metadata checks. Run available Rust tests
  with test/temp paths only; do not invoke real provider-write commands.
- [ ] Start a separate Vite server port and inspect expanded/compact, light/dark,
  home and API-configuration states for rendering, clipping, overflow, console
  errors, and correct Acid Orange identity.
- [ ] Run `codex review --base main` and iterate to no actionable findings.

### Task 5: Commit, push, and package

**Files:**
- Modify only files required by final verification or review.

- [ ] Commit implementation with English conventional messages.
- [ ] Push `codex/橘子下载器` to `origin` and create a PR against `main`.
- [ ] Build the existing Windows NSIS target without installing it.
- [ ] Locate the generated setup EXE, verify it is nonempty and carries the
  Acid Orange icon/artwork, then copy it to the user's Desktop with a clear
  Acid Orange test-build filename.
- [ ] Do not merge while GitHub required checks are blocked by billing.
