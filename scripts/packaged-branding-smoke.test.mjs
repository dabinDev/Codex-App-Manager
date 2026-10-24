import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const scriptsDir = import.meta.dirname;

describe("packaged smoke branding contract", () => {
  it("loads the Windows product name from brand configuration", async () => {
    const windows = await readFile(join(scriptsDir, "windows-packaged-smoke.ps1"), "utf8");

    expect(windows).toContain("brand.config.json");
    expect(windows).not.toMatch(/\[string\]\$ProductName = "Codex App Manager"/);
    expect(windows).toContain('$brand.productName');
  });

  it("loads the macOS product name from brand configuration", async () => {
    const mac = await readFile(join(scriptsDir, "macos-packaged-smoke.sh"), "utf8");

    expect(mac).toContain("brand.config.json");
    expect(mac).toContain("EXPECTED_PRODUCT_NAME");
    expect(mac).not.toContain('[[ "$PRODUCT_NAME" == "Codex App Manager" ]]');
  });
});
