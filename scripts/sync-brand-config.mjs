import { readFile, writeFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import {
  validateAssetFile,
  validateFrontendAssetPath,
} from "./brand-config-lib.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const brandPath = join(repoRoot, "brand.config.json");
const tauriPath = join(repoRoot, "src-tauri", "tauri.conf.json");
const checkOnly = process.argv.includes("--check");

function requiredString(value, field) {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`${field} must be a non-empty string`);
  }
  return value;
}

function requiredUrl(value, field) {
  const raw = requiredString(value, field);
  if (raw !== raw.trim()) {
    throw new Error(`${field} must not contain surrounding whitespace`);
  }
  const parsed = new URL(raw);
  if (parsed.protocol !== "https:") {
    throw new Error(`${field} must use https`);
  }
  if (parsed.username || parsed.password || parsed.hash) {
    throw new Error(`${field} must not contain credentials or a fragment`);
  }
  if (parsed.search) {
    throw new Error(`${field} must not contain a query string`);
  }
  if (raw.endsWith("/")) {
    throw new Error(`${field} must not end with a slash`);
  }
  return raw;
}

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

const brand = await readJson(brandPath);
requiredString(brand.productName, "productName");
requiredString(brand.serviceName, "serviceName");

const locales = ["en", "zh-CN", "zh-TW", "ja", "ko", "fr", "de", "es", "pt-BR", "ru", "ar"];
for (const locale of locales) {
  requiredString(brand.displayNames?.[locale], `displayNames.${locale}`);
}

for (const key of ["support", "service", "mirror", "skins"]) {
  brand.sites[key] = requiredUrl(brand.sites?.[key], `sites.${key}`);
}
if (!Array.isArray(brand.sites?.updater) || brand.sites.updater.length === 0) {
  throw new Error("sites.updater must contain at least one URL");
}
brand.sites.updater = brand.sites.updater.map((url, index) =>
  requiredUrl(url, `sites.updater[${index}]`),
);
brand.updates.artifactBaseUrl = requiredUrl(
  brand.updates?.artifactBaseUrl,
  "updates.artifactBaseUrl",
);

const bundleIcons = brand.icons?.bundle;
if (!Array.isArray(bundleIcons) || bundleIcons.length === 0) {
  throw new Error("icons.bundle must contain at least one icon");
}

const assetPaths = [
  validateFrontendAssetPath(brand.icons?.frontend?.light),
  validateFrontendAssetPath(brand.icons?.frontend?.dark),
  ...bundleIcons.map((path) => join("src-tauri", path)),
  join("src-tauri", requiredString(brand.icons?.windowsInstaller, "icons.windowsInstaller")),
];
for (const assetPath of assetPaths) {
  requiredString(assetPath, "icon path");
  await validateAssetFile(repoRoot, assetPath);
}

const tauri = await readJson(tauriPath);
tauri.productName = brand.productName;
for (const window of tauri.app?.windows ?? []) {
  window.title = brand.productName;
}
tauri.bundle.icon = bundleIcons;
tauri.bundle.windows.nsis.installerIcon = brand.icons.windowsInstaller;
tauri.plugins.updater.endpoints = brand.sites.updater;

const expected = `${JSON.stringify(tauri, null, 2)}\n`;
const current = await readFile(tauriPath, "utf8");
if (current === expected) {
  console.log("Brand-derived Tauri configuration is current.");
} else if (checkOnly) {
  throw new Error(
    `${relative(repoRoot, tauriPath)} is out of sync; run npm run brand:sync`,
  );
} else {
  await writeFile(tauriPath, expected);
  console.log(`Updated ${relative(repoRoot, tauriPath)} from brand.config.json.`);
}
