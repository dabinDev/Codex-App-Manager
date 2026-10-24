# Signed Branded Windows Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fail-closed Windows distribution command that only archives and delivers acid-orange or rotten-tomato installers after trusted Authenticode signing and byte-for-byte verification.

**Architecture:** Keep ordinary `tauri:build` and optional signing unchanged for development. Add a staged PowerShell orchestrator with injectable build/sign/verify adapters for tests; production mode requires PFX credentials, validates brand and commit identity, signs the final NSIS bytes, verifies metadata and hashes, archives them, then performs a backup-first desktop copy.

**Tech Stack:** PowerShell 7, Authenticode/SignTool, Tauri v2 NSIS, Node/Vitest static contracts, Git worktrees, SHA-256.

---

### Task 1: Make required PFX signing explicit and testable

**Files:**
- Modify: `scripts/sign-windows-authenticode.ps1`
- Create: `scripts/tests/sign-windows-authenticode.Tests.ps1`

- [ ] **Step 1: Write failing Pester tests**

Cover optional missing-certificate success and required missing-certificate failure without importing any real certificate:

```powershell
It "fails when required signing has no certificate" {
    { & $script -Path $fixture -Mode required -CertificateBase64 "" } |
        Should -Throw "*[sign]*certificate*required*"
}

It "keeps optional development signing non-blocking" {
    { & $script -Path $fixture -Mode optional -CertificateBase64 "" } |
        Should -Not -Throw
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests/sign-windows-authenticode.Tests.ps1 -Output Detailed"`

Expected: parameter binding fails because `-Mode` does not exist.

- [ ] **Step 3: Add required mode**

Add `[ValidateSet("optional", "required")] [string]$Mode = "optional"`. In required mode, reject an empty certificate before creating temporary files; preserve existing optional skip behavior. Keep secret values out of logs and retain cleanup in `finally`.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests/sign-windows-authenticode.Tests.ps1 -Output Detailed"`

Expected: both required and optional behavior tests pass.

- [ ] **Step 5: Commit Task 1**

```bash
git add scripts/sign-windows-authenticode.ps1 scripts/tests/sign-windows-authenticode.Tests.ps1
git commit -m "fix: require certificate for distribution signing"
```

### Task 2: Add a staged distributable delivery orchestrator

**Files:**
- Create: `scripts/build-windows-distributable.ps1`
- Create: `scripts/tests/build-windows-distributable.Tests.ps1`
- Modify: `package.json`

- [ ] **Step 1: Write failing preflight and no-overwrite tests**

Use `TestDrive:` fixtures and injected script blocks so no real build, signature, or desktop path is touched:

```powershell
It "leaves an existing delivery untouched when signing preflight fails" {
    Set-Content $delivery "old-installer"
    { & $script -Brand acid-orange -ExpectedCommit $commit -DeliveryDirectory $out `
        -CertificateBase64 "" -SkipBuildForTest } | Should -Throw "*[preflight]*"
    (Get-Content $delivery -Raw) | Should -Be "old-installer"
}

It "rejects a worktree that is not based on the requested commit" {
    { & $script -Brand acid-orange -ExpectedCommit deadbeef -Worktree $fixture } |
        Should -Throw "*[preflight]*commit*"
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests/build-windows-distributable.Tests.ps1 -Output Detailed"`

Expected: failure because the orchestrator does not exist.

- [ ] **Step 3: Implement parameter and stage contracts**

Define validated parameters for `Brand`, `Worktree`, `ExpectedCommit`, `ArtifactRoot`, `DeliveryDirectory`, `ExpectedPublisher`, and the existing certificate variables. Centralize failure messages:

```powershell
function Fail-Stage([string]$Stage, [string]$Message, [string]$Path = "") {
    $suffix = if ($Path) { " path=$Path" } else { "" }
    throw "[$Stage][$Brand] $Message$suffix"
}
```

Use only these stable stages: `preflight`, `brand-sync`, `build`, `sign`, `sign-verify`, `archive`, `desktop-copy`.

- [ ] **Step 4: Implement preflight and brand validation**

Resolve every path, require the worktree HEAD to equal `ExpectedCommit`, allow only the expected brand-config delta in an isolated worktree, require `brand.config.json` values to match the selected brand table, and run `npm run brand:sync` plus `npm run check`. Never mutate `main` to switch brands.

- [ ] **Step 5: Implement the production build gate**

Run the existing Tauri production build with `tauri/custom-protocol` and locate exactly one x64 NSIS installer for `productName` and version. The script must not install or execute it. Test adapters may bypass the expensive build only when an explicit internal test switch is supplied by the Pester harness.

- [ ] **Step 6: Implement required signing and verification**

Call:

```powershell
& $signScript -Path $installer -Mode required -CertificateBase64 $CertificateBase64 `
  -CertificatePassword $CertificatePassword -TimestampUrl $TimestampUrl -Stage sign
& $verifyScript -Path $installer -Mode required -ExpectedSubject $ExpectedPublisher `
  -Stage sign-verify
```

Additionally require a timestamp, x64 architecture, expected product name/version, embedded `/index.html`, brand service URL, and production custom protocol evidence. A missing tool or unverifiable property fails closed.

- [ ] **Step 7: Run Task 2 preflight tests and verify GREEN**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests/build-windows-distributable.Tests.ps1 -Output Detailed"`

Expected: preflight failures contain stable stages and leave delivery fixtures unchanged.

- [ ] **Step 8: Commit Task 2**

```bash
git add scripts/build-windows-distributable.ps1 scripts/tests/build-windows-distributable.Tests.ps1 package.json
git commit -m "feat: add signed Windows distribution command"
```

### Task 3: Make archive and desktop delivery transactional

**Files:**
- Modify: `scripts/build-windows-distributable.ps1`
- Modify: `scripts/tests/build-windows-distributable.Tests.ps1`

- [ ] **Step 1: Write failing archive and copy tests**

Cover source/archive hash mismatch, archive write failure, backup creation, desktop copy hash mismatch, and successful delivery:

```powershell
It "backs up then delivers identical verified bytes" {
    Invoke-TestDelivery -ExistingDesktopBytes "old" -SignedBytes "signed"
    (Get-FileHash $source -Algorithm SHA256).Hash | Should -Be `
        (Get-FileHash $archive -Algorithm SHA256).Hash
    (Get-FileHash $source -Algorithm SHA256).Hash | Should -Be `
        (Get-FileHash $delivery -Algorithm SHA256).Hash
    $backup | Should -Exist
}

It "does not replace desktop output when archive hash differs" {
    { Invoke-TestDelivery -TamperArchive } | Should -Throw "*[archive]*SHA-256*"
    (Get-Content $delivery -Raw) | Should -Be "old"
}
```

- [ ] **Step 2: Run focused tests and verify RED**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests/build-windows-distributable.Tests.ps1 -Tag delivery -Output Detailed"`

Expected: tests fail because archive and delivery are not implemented.

- [ ] **Step 3: Implement commit-scoped archive delivery**

Write only to `<ArtifactRoot>/<commit>/<brand>/signed`. Copy the signed installer to a temporary file in that directory, verify its SHA-256 against the source, then atomically rename it to the final archive name.

- [ ] **Step 4: Implement backup-first desktop replacement**

If the requested desktop filename exists, copy it to a timestamped `.bak` sibling and verify the backup hash before replacing anything. Copy the archive to a temporary desktop sibling, verify SHA-256, then atomically replace the requested filename. On any failure remove only temporary files created by this run; preserve the old delivery and its backup.

- [ ] **Step 5: Emit a structured success record**

Return a JSON object containing only `productName`, `version`, `publisher`, `commit`, `archivePath`, `deliveryPath`, and `sha256`. Do not emit PFX bytes, passwords, or certificate private-key details.

- [ ] **Step 6: Run all delivery tests and verify GREEN**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests/build-windows-distributable.Tests.ps1 -Output Detailed"`

Expected: every failure preserves the existing desktop fixture and success hashes all match.

- [ ] **Step 7: Commit Task 3**

```bash
git add scripts/build-windows-distributable.ps1 scripts/tests/build-windows-distributable.Tests.ps1
git commit -m "feat: verify Windows installer delivery bytes"
```

### Task 4: Enforce and document the distribution policy

**Files:**
- Modify: `docs/windows-signing.md`
- Modify: `.github/workflows/release.yml`
- Create: `scripts/windows-distribution-contract.test.mjs`
- Modify: `scripts/release-workflow.test.mjs`

- [ ] **Step 1: Write failing workflow and static policy tests**

Assert that stable release signing happens before updater `.sig` generation/upload, required verification is controlled explicitly, and the distributable command never copies before verification:

```js
expect(distribution.indexOf("-Mode required")).toBeGreaterThan(-1);
expect(distribution.indexOf("sign-verify")).toBeLessThan(distribution.indexOf("desktop-copy"));
expect(workflow.indexOf("sign-windows-authenticode.ps1")).toBeLessThan(
  workflow.indexOf("tauri signer sign"),
);
```

- [ ] **Step 2: Run tests and verify RED**

Run: `npx vitest run scripts/windows-distribution-contract.test.mjs scripts/release-workflow.test.mjs`

Expected: policy assertions fail until workflow ordering and docs are updated.

- [ ] **Step 3: Update workflow policy without claiming a certificate exists**

Keep normal PR/development verification optional. For trusted stable releases, make `AUTHENTICODE_REQUIRED=true` enforce required verification; when the secret is absent the release must fail rather than upload unsigned bytes. Generate updater signatures only after final Authenticode bytes are verified.

- [ ] **Step 4: Update signing documentation**

Document the ordinary development command, fail-closed distributable command, both brand worktree requirement, archive layout, desktop backup behavior, PFX environment variables, SignPath as pending future work, and the current external blocker. State clearly that Smart App Control must not be disabled and self-signed roots are unsupported.

- [ ] **Step 5: Run policy tests and verify GREEN**

Run: `npx vitest run scripts/windows-distribution-contract.test.mjs scripts/release-workflow.test.mjs`

Expected: workflow and distribution policy tests pass without accessing secrets.

- [ ] **Step 6: Commit Task 4**

```bash
git add docs/windows-signing.md .github/workflows/release.yml scripts/windows-distribution-contract.test.mjs scripts/release-workflow.test.mjs
git commit -m "docs: enforce signed Windows distribution policy"
```

### Task 5: Verify fail-closed behavior and complete repository gates

**Files:**
- Verify all files from Tasks 1-4

- [ ] **Step 1: Prove missing credentials block delivery**

Run the distributable command against a disposable output directory with no `WINDOWS_CERTIFICATE`:

```powershell
$env:WINDOWS_CERTIFICATE = ""
pwsh -NoProfile -File scripts/build-windows-distributable.ps1 `
  -Brand acid-orange -Worktree $PWD -ExpectedCommit (git rev-parse HEAD) `
  -DeliveryDirectory (Join-Path $env:TEMP "cam-delivery-proof")
```

Expected: non-zero failure at `[preflight]` or `[sign]`; no installer is copied. Do not point this proof at the real desktop.

- [ ] **Step 2: Run all automated tests**

Run: `pwsh -NoProfile -Command "Invoke-Pester scripts/tests -Output Detailed"`

Run: `npm run check && npm run lint && npm test && npm run build`

Run: `cargo test --manifest-path crates/codex-theme-engine/Cargo.toml --all-targets`

Run: `cargo test --manifest-path src-tauri/Cargo.toml --all-targets`

Run: `git diff --check`

Expected: all tests pass; no test installs or runs a generated EXE and no real desktop file changes.

- [ ] **Step 3: Run the repository review gate**

Run: `codex review --uncommitted`

Expected: no actionable findings. Address findings and rerun until clean.

- [ ] **Step 4: Commit final corrections and push**

```bash
git add scripts docs .github package.json src crates src-tauri
git commit -m "feat: require signed branded Windows deliverables"
git push origin main
```

Expected: push succeeds without force. If no trusted certificate is configured, report that real signed desktop packages remain blocked; do not copy unsigned installers to the desktop.
