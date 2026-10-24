#!/usr/bin/env node
// Build the Tauri updater manifest (latest.json) from collected release
// artifacts. Each macOS updater tarball is renamed with its arch during the
// CI "Collect artifacts" step and carries a sibling .sig; we read those sigs
// and point the urls at the GitHub release download path. The manifest is
// served as a release asset, matching the updater endpoints in tauri.conf.json.
//
// Usage: node scripts/gen-updater-manifest.mjs <tag> <artifacts-dir>
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { artifactDownloadUrl, buildUpdaterManifest } from "./updater-manifest-lib.mjs";

const [, , tag, dir] = process.argv;
if (!tag || !dir) {
  console.error("usage: gen-updater-manifest.mjs <tag> <artifacts-dir>");
  process.exit(2);
}
const version = tag.replace(/^v/, "");
const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const brand = JSON.parse(readFileSync(join(repoRoot, "brand.config.json"), "utf8"));

const files = readdirSync(dir);
const findSig = (re) => files.find((f) => re.test(f) && f.endsWith(".sig"));
const sha256File = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

// Tauri updater platform keys → how to spot that platform's signed bundle.
const MATCHERS = [
  ["darwin-aarch64", /aarch64.*\.app\.tar\.gz\.sig$/],
  ["darwin-x86_64", /x86_64.*\.app\.tar\.gz\.sig$/],
  ["windows-x86_64", /(?:x64|x86_64).*-setup\.(?:exe|nsis\.zip)\.sig$/],
  ["windows-aarch64", /(?:arm64|aarch64).*-setup\.(?:exe|nsis\.zip)\.sig$/],
];
const REQUIRED_PLATFORMS = MATCHERS.map(([key]) => key);
const allowPartialRelease = process.env.ALLOW_PARTIAL_RELEASE === "1";

const platformInputs = {};
const resolved = [];
for (const [key, re] of MATCHERS) {
  const sig = findSig(re);
  if (!sig) {
    console.warn(`[manifest] no signed artifact for ${key} — skipping`);
    continue;
  }
  const bundle = sig.replace(/\.sig$/, "");
  const bundlePath = join(dir, bundle);
  if (!existsSync(bundlePath)) {
    console.error(`[manifest] signature ${sig} resolved ${bundle}, but the bundle is missing`);
    process.exit(1);
  }
  const url = artifactDownloadUrl(brand.updates.artifactBaseUrl, tag, bundle);
  platformInputs[key] = {
    signature: readFileSync(join(dir, sig), "utf8").trim(),
    bundle,
  };
  resolved.push({
    platform: key,
    artifact: bundle,
    signature_file: sig,
    sha256: sha256File(bundlePath),
    url,
  });
}

if (Object.keys(platformInputs).length === 0) {
  console.error("[manifest] no platforms resolved — check artifact globs");
  process.exit(1);
}

const missing = REQUIRED_PLATFORMS.filter((key) => !platformInputs[key]);
if (missing.length > 0) {
  const message = `[manifest] missing required platform artifacts: ${missing.join(", ")}`;
  if (!allowPartialRelease) {
    console.error(message);
    console.error("[manifest] set ALLOW_PARTIAL_RELEASE=1 only for an intentional one-off partial release");
    process.exit(1);
  }
  console.warn(`::warning::partial release allowed: missing ${missing.join(", ")}`);
}

const manifest = buildUpdaterManifest({
  tag,
  productName: brand.productName,
  artifactBaseUrl: brand.updates.artifactBaseUrl,
  pubDate: new Date().toISOString(),
  platforms: platformInputs,
});
if (missing.length > 0) {
  manifest.partial = true;
  manifest.missing = missing;
}

writeFileSync("latest.json", JSON.stringify(manifest, null, 2));
writeFileSync(
  "manifest-summary.json",
  JSON.stringify(
    {
      tag,
      version,
      allow_partial_release: allowPartialRelease,
      partial: missing.length > 0,
      required_platforms: REQUIRED_PLATFORMS.map((platform) => ({
        platform,
        present: Boolean(platformInputs[platform]),
        missing: !platformInputs[platform],
      })),
      missing,
      artifacts: resolved,
    },
    null,
    2,
  ) + "\n",
);
console.log("wrote latest.json:\n" + JSON.stringify(manifest, null, 2));
