import { describe, expect, it } from "vitest";

import { buildUpdaterManifest } from "./updater-manifest-lib.mjs";

describe("updater manifest branding", () => {
  it("uses configured product and artifact base URLs", () => {
    const manifest = buildUpdaterManifest({
      tag: "v9.8.7",
      productName: "Example Installer",
      artifactBaseUrl: "https://downloads.example.test/apps",
      pubDate: "2026-08-01T00:00:00.000Z",
      platforms: {
        "windows-x86_64": {
          bundle: "example setup.exe",
          signature: "signed",
        },
      },
    });

    expect(manifest).toEqual({
      version: "9.8.7",
      notes: "Example Installer v9.8.7",
      pub_date: "2026-08-01T00:00:00.000Z",
      platforms: {
        "windows-x86_64": {
          signature: "signed",
          url: "https://downloads.example.test/apps/v9.8.7/example%20setup.exe",
        },
      },
    });
  });
});
