import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const scriptsDir = dirname(fileURLToPath(import.meta.url));

describe("packaged alignment smoke fixtures", () => {
  it.each(["windows-packaged-smoke.ps1", "macos-packaged-smoke.sh"])(
    "%s preserves one malformed rollout by hash",
    async (scriptName) => {
      const script = await readFile(join(scriptsDir, scriptName), "utf8");

      expect(script).toContain("rollout-invalid-packaged-smoke.jsonl");
      expect(script).toMatch(/sha256|SHA256/);
    },
  );

  it("keeps the Windows snapshot helper compatible with Windows PowerShell", async () => {
    const script = await readFile(
      join(scriptsDir, "windows-packaged-smoke.ps1"),
      "utf8",
    );

    expect(script).not.toMatch(/foreach\s*\(\s*\$home\s+in\b/i);
    expect(script).not.toContain("[Convert]::ToHexString");
    expect(script).not.toContain("[System.IO.Path]::IsPathFullyQualified");
    expect(script).toContain("function Get-SharedFileHash");
    expect(script).toContain("[System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete");
    expect(script).toContain("Hash = Get-SharedFileHash $databasePath");
    expect(script).not.toContain("Get-FileHash -LiteralPath $databasePath");
    expect(script).toContain('foreach ($sidecarSuffix in @("-wal", "-shm", "-journal"))');
    expect(script).toMatch(
      /Get-ChildItem\s+-LiteralPath\s+\$databaseRoot\s+-Filter\s+"state_\*\.sqlite"\s+-File\s+-Force\s+-ErrorAction\s+Stop/,
    );
    expect(script).toMatch(
      /Get-ChildItem\s+-LiteralPath\s+\$rolloutRoot\s+-Filter\s+"rollout-\*\.jsonl"\s+-File\s+-Recurse\s+-Force\s+-ErrorAction\s+Stop/,
    );
    expect(script).toContain('Join-Path $codexHome "recovery-backups"');
    expect(script).toMatch(
      /Get-ChildItem\s+-LiteralPath\s+\$recoveryRoot\s+-Recurse\s+-Force\s+-ErrorAction\s+Stop/,
    );
  });

  it("passes embedded Python fixtures through stdin on Windows PowerShell", async () => {
    const script = await readFile(
      join(scriptsDir, "windows-packaged-smoke.ps1"),
      "utf8",
    );

    expect(script).not.toContain("@script:SmokePythonPrefix -c $sqliteFixture");
    expect(script).not.toContain("@script:SmokePythonPrefix -c $verifyFixture");
    expect(script).toMatch(
      /\$sqliteFixture\s*\|\s*&\s*\$script:SmokePython\s+@script:SmokePythonPrefix\s+-\s+\$Database/,
    );
    expect(script).toMatch(
      /\$verifyFixture\s*\|\s*&\s*\$script:SmokePython\s+@script:SmokePythonPrefix\s+-\s+`?\s*\$Database/,
    );
  });
});
