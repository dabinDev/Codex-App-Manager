import brandConfig from "../../brand.config.json";

import type { Lang } from "./i18n";

export const BRAND = brandConfig;
export const BRAND_SERVICE_NAME =
  import.meta.env.VITE_BRAND_SERVICE_NAME?.trim() || BRAND.serviceName;

export function brandName(lang: Lang): string {
  return BRAND.displayNames[lang];
}

export function siteHost(url: string): string {
  return new URL(url).host;
}

const frontendAssets = import.meta.glob("./assets/**/*", {
  eager: true,
  import: "default",
  query: "?url",
}) as Record<string, string>;

function frontendAsset(configPath: string): string {
  const key = configPath.replace(/^src\/app\//, "./");
  const asset = frontendAssets[key];
  if (!asset) {
    throw new Error(`Brand asset is not bundled: ${configPath}`);
  }
  return asset;
}

export const BRAND_LOGO_LIGHT = frontendAsset(BRAND.icons.frontend.light);
export const BRAND_LOGO_DARK = frontendAsset(BRAND.icons.frontend.dark);
