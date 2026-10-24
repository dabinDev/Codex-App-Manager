import { describe, expect, it } from "vitest";

import cargoToml from "../../src-tauri/Cargo.toml?raw";
import rustLib from "../../src-tauri/src/lib.rs?raw";

describe("Windows WebView2 ABI dependencies", () => {
  it("transfers the Tauri/Wry COM handle into the upgraded bindings", () => {
    expect(cargoToml).toContain('webview2-com = "=0.39.1"');
    expect(cargoToml).toContain('windows-core = "0.62.2"');
    expect(cargoToml).toContain(
      'windows-core-wry = { package = "windows-core", version = "=0.61.2" }',
    );
    expect(cargoToml).not.toContain("webview2-com-wry");
    expect(rustLib).toContain("use webview2_com::");
    expect(rustLib).toContain("use windows_core::Interface;");
    expect(rustLib).toContain("use windows_core_wry::Interface as WryInterface;");
    expect(rustLib).toContain("WryInterface::into_raw(legacy_settings)");
    expect(rustLib).toContain("ICoreWebView2Settings::from_raw(raw_settings)");
  });
});
