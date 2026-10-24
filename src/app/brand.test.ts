import { describe, expect, it } from "vitest";

import {
  BRAND,
  BRAND_LOGO_DARK,
  BRAND_LOGO_LIGHT,
  BRAND_SERVICE_NAME,
  brandName,
  siteHost,
} from "./brand";
import type { Lang } from "./i18n";

const LANGS: Lang[] = [
  "en",
  "zh-CN",
  "zh-TW",
  "ja",
  "ko",
  "fr",
  "de",
  "es",
  "pt-BR",
  "ru",
  "ar",
];

describe("brand configuration", () => {
  it("provides every supported display name", () => {
    expect(BRAND_SERVICE_NAME.trim()).not.toBe("");
    for (const lang of LANGS) {
      expect(brandName(lang).trim()).not.toBe("");
    }
  });

  it("uses valid HTTPS sites", () => {
    const urls = [
      BRAND.sites.support,
      BRAND.sites.service,
      BRAND.sites.mirror,
      BRAND.sites.skins,
      ...BRAND.sites.updater,
    ];
    for (const url of urls) {
      expect(new URL(url).protocol).toBe("https:");
      expect(url).not.toMatch(/\/$/);
    }
    expect(siteHost(BRAND.sites.service)).toBe(new URL(BRAND.sites.service).host);
  });

  it("bundles both configured frontend logos", () => {
    expect(BRAND_LOGO_LIGHT).toBeTruthy();
    expect(BRAND_LOGO_DARK).toBeTruthy();
  });
});
