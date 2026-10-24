import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const approvedLogoHash = "63578c27b64b70c361aa80d303691d50439aeff3a7da1e4979ad367d5dc58357";

function repoFile(path) {
  return readFileSync(join(repoRoot, path));
}

function containsBytes(source, bytes) {
  return source.indexOf(Buffer.from(bytes)) >= 0;
}

describe("Acid Orange brand contract", () => {
  it("uses the approved Acid Orange artwork byte-for-byte", () => {
    for (const path of [
      "src/app/assets/orange-logo.png",
      "public/favicon.png",
      "assets/icon.icon/Assets/logo.png",
    ]) {
      const logoPath = join(repoRoot, path);
      expect(existsSync(logoPath), path).toBe(true);
      const hash = createHash("sha256").update(readFileSync(logoPath)).digest("hex");
      expect(hash, path).toBe(approvedLogoHash);
    }

    const html = repoFile("index.html").toString("utf8");
    expect(html).toContain('<link rel="icon" type="image/png" href="/favicon.png" />');
  });

  it("keeps the default remote login host configuration-driven", () => {
    const brand = JSON.parse(repoFile("brand.config.json").toString("utf8"));
    const rust = repoFile("src-tauri/src/app/orange_api.rs").toString("utf8");

    expect(brand.sites.service).toBe("https://token.cylonai.cn");
    expect(rust).toContain("use crate::app::brand::SERVICE_BASE_URL;");
    expect(rust).toContain("pub const ORANGE_BASE_URL: &str = SERVICE_BASE_URL;");
    expect(rust).not.toContain(
      'pub const ORANGE_BASE_URL: &str = "https://token.cylonai.cn";',
    );
  });

  it("keeps required generated native icon outputs", () => {
    for (const path of [
      "src-tauri/icons/32x32.png",
      "src-tauri/icons/128x128.png",
      "src-tauri/icons/128x128@2x.png",
      "src-tauri/icons/icon.png",
      "src-tauri/icons/icon.ico",
      "src-tauri/icons/icon.icns",
    ]) {
      expect(existsSync(join(repoRoot, path)), path).toBe(true);
    }

    expect(existsSync(join(repoRoot, "src-tauri/macos/Assets.car"))).toBe(false);
  });

  it("brands both NSIS bitmaps with the approved orange and teal anchors", () => {
    for (const path of [
      "src-tauri/installer/header.bmp",
      "src-tauri/installer/sidebar.bmp",
    ]) {
      const bitmap = repoFile(path);
      expect(bitmap.subarray(0, 2).toString("ascii"), path).toBe("BM");
      expect(containsBytes(bitmap, [0x1a, 0x6b, 0xff]), `${path} orange`).toBe(true);
      expect(containsBytes(bitmap, [0x83, 0x8f, 0x0f]), `${path} teal`).toBe(true);
    }
  });
});
