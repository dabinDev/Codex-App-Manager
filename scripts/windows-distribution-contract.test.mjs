import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = join(import.meta.dirname, "..");

describe("signed Windows distribution contract", () => {
  it("exposes only the fail-closed delivery script as the distribution command", async () => {
    const packageJson = JSON.parse(await readFile(join(repoRoot, "package.json"), "utf8"));
    expect(packageJson.scripts["windows:distribute"]).toBe(
      "pwsh -NoProfile -File scripts/build-windows-distributable.ps1",
    );

    const script = await readFile(
      join(repoRoot, "scripts", "build-windows-distributable.ps1"),
      "utf8",
    );
    expect(script).toContain("-Mode required");
    expect(script.indexOf('"sign-verify"')).toBeLessThan(script.indexOf("Copy-ToVerifiedArchive"));
    expect(script.indexOf("Copy-ToVerifiedArchive")).toBeLessThan(
      script.indexOf("Publish-VerifiedDelivery"),
    );
    expect(script).toContain("build --no-bundle --target $rustTarget");
    expect(script).toContain("bundle --bundles nsis --target $rustTarget");
    expect(script).toContain('Join-Path $targetRoot "codex-app-manager.exe"');
    expect(script).toContain("[Diagnostics.FileVersionInfo]::GetVersionInfo");

    const signMain = script.indexOf("-Path $mainExecutable.FullName");
    const bundle = script.indexOf("bundle --bundles nsis");
    const signInstaller = script.indexOf("-Path $installer.FullName");
    expect(signMain).toBeGreaterThan(-1);
    expect(signMain).toBeLessThan(bundle);
    expect(bundle).toBeLessThan(signInstaller);
    expect(script.lastIndexOf("verify-windows-authenticode.ps1")).toBeGreaterThan(
      script.indexOf("Copy-ToVerifiedArchive"),
    );
  });

  it("signs Authenticode bytes before producing the updater signature", async () => {
    const workflow = await readFile(join(repoRoot, ".github", "workflows", "release.yml"), "utf8");
    const authenticode = workflow.indexOf("sign-windows-authenticode.ps1");
    const updater = workflow.indexOf('npx tauri signer sign "$exe"');
    expect(authenticode).toBeGreaterThan(-1);
    expect(authenticode).toBeLessThan(updater);
    expect(workflow).toContain("-Mode $env:AUTHENTICODE_MODE");
  });

  it("requires an exact non-empty signer subject for distributable output", async () => {
    const buildScript = await readFile(
      join(repoRoot, "scripts", "build-windows-distributable.ps1"),
      "utf8",
    );
    const verifyScript = await readFile(
      join(repoRoot, "scripts", "verify-windows-authenticode.ps1"),
      "utf8",
    );

    expect(buildScript).toContain("expected publisher identity is required");
    expect(verifyScript).toContain("$subject -ne $ExpectedSubject");
    expect(verifyScript).not.toContain('-notlike "*$ExpectedSubject*"');
  });

  it("documents the certificate blocker without recommending trust bypasses", async () => {
    const docs = await readFile(join(repoRoot, "docs", "windows-signing.md"), "utf8");
    expect(docs).toContain("windows:distribute");
    expect(docs).toContain("Smart App Control");
    expect(docs).toContain("must not be disabled");
    expect(docs).toContain("self-signed root");
  });
});
