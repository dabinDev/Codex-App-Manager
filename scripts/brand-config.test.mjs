import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  validateAssetFile,
  validateFrontendAssetPath,
} from "./brand-config-lib.mjs";

const repoRoot = join(import.meta.dirname, "..");

describe("brand frontend asset contract", () => {
  it("accepts nested assets and rejects paths outside the asset root", () => {
    expect(validateFrontendAssetPath("src/app/assets/acme/logo.png")).toBe(
      "src/app/assets/acme/logo.png",
    );
    expect(() => validateFrontendAssetPath("src/app/other/logo.png")).toThrow(
      /must be below src\/app\/assets/,
    );
    expect(() => validateFrontendAssetPath("src/app/assets/../secret.png")).toThrow(
      /must be below src\/app\/assets/,
    );
  });

  it("uses the same recursive asset scope at runtime", async () => {
    const source = await readFile(join(repoRoot, "src/app/brand.ts"), "utf8");
    expect(source).toContain('import.meta.glob("./assets/**/*"');
  });

  it("rejects a directory used as a frontend asset", async () => {
    await expect(validateAssetFile(repoRoot, "src/app/assets")).rejects.toThrow(
      /regular file/,
    );
  });
});
