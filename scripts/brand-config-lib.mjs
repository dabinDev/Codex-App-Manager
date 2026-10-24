import { stat } from "node:fs/promises";
import { join, posix } from "node:path";

const FRONTEND_ASSET_ROOT = "src/app/assets/";

export function validateFrontendAssetPath(value) {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error("frontend icon path must be a non-empty string");
  }
  const normalized = posix.normalize(value);
  if (
    value !== value.trim() ||
    value.includes("\\") ||
    normalized !== value ||
    !normalized.startsWith(FRONTEND_ASSET_ROOT)
  ) {
    throw new Error(`frontend icon path must be below ${FRONTEND_ASSET_ROOT}`);
  }
  return normalized;
}

export async function validateAssetFile(repoRoot, assetPath) {
  const metadata = await stat(join(repoRoot, assetPath));
  if (!metadata.isFile()) {
    throw new Error(`${assetPath} must be a regular file`);
  }
  return assetPath;
}
