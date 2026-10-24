import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_SETTINGS } from "../shared/types";
import { isNetworkError, managerApi, SETTINGS_CHANGED_EVENT } from "./managerApi";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  vi.stubGlobal("window", { open: vi.fn(), __TAURI_INTERNALS__: undefined });
  localStorage.clear();
});

describe("API configuration IPC", () => {
  const session = {
    authenticated: true,
    email: "user@example.com",
    remembered: true,
    connection: "connected" as const,
    warning: null,
  };
  const keys = {
    items: [
      {
        id: 41,
        name: "Primary",
        groupName: "OpenAI",
        maskedKey: "sk-a*****xyz",
        status: "active" as const,
        quota: 0,
        quotaUsed: 1,
        expiresAt: null,
        actionable: true,
        enabled: false,
      },
    ],
    stale: false,
    fetchedAtUnix: 1,
  };

  it("invokes key actions by ID without returning full secrets to the renderer", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock
      .mockResolvedValueOnce(session)
      .mockResolvedValueOnce(keys)
      .mockResolvedValueOnce(undefined);

    await managerApi.apiConfigSession();
    await managerApi.apiConfigKeys();
    await managerApi.apiConfigImportCcs(41);

    expect(invokeMock).toHaveBeenNthCalledWith(1, "api_config_session");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "api_config_keys");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "api_config_import_ccs", { keyId: 41 });
    expect(JSON.stringify(invokeMock.mock.calls)).not.toContain("sk-secret");
  });

  it("rejects malformed key statuses at the IPC boundary", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue({
      ...keys,
      items: [{ ...keys.items[0], status: "mystery" }],
    });

    await expect(managerApi.apiConfigKeys()).rejects.toMatchObject({
      code: "contract_mismatch",
    });
  });

  it("keeps browser preview signed out and blocks mutating actions", async () => {
    await expect(managerApi.apiConfigSession()).resolves.toEqual({
      authenticated: false,
      email: null,
      remembered: false,
      connection: "signed_out",
      warning: "desktop_required",
    });
    await expect(managerApi.apiConfigLogin("a@b.test", "pw", true)).rejects.toMatchObject({
      code: "desktop_required",
    });
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

describe("manager self-update IPC", () => {
  it("preserves categorized update-check failures", async () => {
    window.__TAURI_INTERNALS__ = {};
    const failure = { code: "network", message: "check manager update failed" };
    invokeMock.mockRejectedValueOnce(failure);

    await expect(managerApi.checkManagerUpdate()).rejects.toEqual(failure);
    expect(invokeMock).toHaveBeenCalledWith("manager_check_update");
  });
});

describe("Toolbox conversation alignment IPC", () => {
  const scan = {
    scanToken: "scan-token",
    activeProvider: "OpenAI",
    databaseCount: 2,
    threadCount: 24,
    mismatchedThreads: 3,
    rolloutCount: 8,
    mismatchedRollouts: 2,
    skippedRollouts: 1,
    skipReasons: ["alignment_rollout_invalid_json"],
    desktopRunning: true,
    cliRunning: false,
    canExecute: true,
    warnings: [],
  };
  const report = {
    outcome: "committed" as const,
    recoveryId: "provider-align-1",
    backupDir: "C:/tmp/provider-align-1",
    threadsUpdated: 3,
    rolloutsUpdated: 2,
    rolloutsSkipped: 1,
    writeVerified: true,
    rollbackVerified: false,
    codexWasRunning: true,
    restartAllowed: true,
    undoAllowed: true,
    elevationRequired: false,
    errorCode: null,
  };

  it("uses exact command names and camelCase payload keys", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock
      .mockResolvedValueOnce(scan)
      .mockResolvedValueOnce(report)
      .mockResolvedValueOnce(report)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);

    await managerApi.toolboxAlignmentScan();
    await managerApi.toolboxAlignmentExecute("scan-token");
    await managerApi.toolboxAlignmentRestore("provider-align-1");
    await managerApi.toolboxAlignmentOpenBackup("provider-align-1");
    await managerApi.toolboxAlignmentRestart();
    await managerApi.toolboxForceRestartCodex(true);
    await managerApi.toolboxApplyCodexLocale(true);

    expect(invokeMock).toHaveBeenNthCalledWith(1, "toolbox_alignment_scan");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "toolbox_alignment_execute", {
      scanToken: "scan-token",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(3, "toolbox_alignment_restore", {
      recoveryId: "provider-align-1",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "toolbox_alignment_open_backup", {
      recoveryId: "provider-align-1",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(5, "toolbox_alignment_restart");
    expect(invokeMock).toHaveBeenNthCalledWith(6, "toolbox_force_restart_codex", {
      confirmed: true,
    });
    expect(invokeMock).toHaveBeenNthCalledWith(7, "toolbox_apply_codex_locale", {
      confirmed: true,
    });
  });

  it("rejects malformed scan and report contracts", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock
      .mockResolvedValueOnce({ ...scan, threadCount: -1 })
      .mockResolvedValueOnce({ ...report, outcome: "unknown" });

    await expect(managerApi.toolboxAlignmentScan()).rejects.toMatchObject({
      code: "contract_mismatch",
    });
    await expect(managerApi.toolboxAlignmentExecute("scan-token")).rejects.toMatchObject({
      code: "contract_mismatch",
    });
  });

  it.each([
    ["negative skipped count", { skippedRollouts: -1 }],
    ["fractional skipped count", { skippedRollouts: 0.5 }],
    ["non-string skip reason", { skipReasons: [42] }],
  ])("rejects %s in scan contracts", async (_name, invalid) => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue({ ...scan, ...invalid });

    await expect(managerApi.toolboxAlignmentScan()).rejects.toMatchObject({
      code: "contract_mismatch",
    });
  });

  it.each([
    ["negative skipped count", { rolloutsSkipped: -1 }],
    ["fractional skipped count", { rolloutsSkipped: 0.5 }],
    ["non-boolean elevation flag", { elevationRequired: "yes" }],
  ])("rejects %s in report contracts", async (_name, invalid) => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue({ ...report, ...invalid });

    await expect(managerApi.toolboxAlignmentExecute("scan-token")).rejects.toMatchObject({
      code: "contract_mismatch",
    });
  });
});

describe("isNetworkError", () => {
  it("classifies transport and TLS failures as connectivity errors", () => {
    expect(
      isNetworkError(
        "update engine error: io error: curl failed for host=codexapp.agentsmirror.com exit=35: stderr='curl: (35) schannel: failed to receive handshake, SSL/TLS connection failed'",
      ),
    ).toBe(true);
    expect(isNetworkError("curl: (6) Could not resolve host: codexapp.agentsmirror.com")).toBe(
      true,
    );
    expect(isNetworkError("curl: (28) Operation timed out after 20000 milliseconds")).toBe(true);
  });

  it("classifies the macOS auto-source fallback failure as connectivity", () => {
    expect(
      isNetworkError("both the mirror and OpenAI official appcast are unreachable"),
    ).toBe(true);
  });

  it("does not treat server responses or verification failures as connectivity", () => {
    expect(
      isNetworkError(
        "update engine error: curl failed for https://example.test/appcast.xml: curl: (22) The requested URL returned error: 404",
      ),
    ).toBe(false);
    expect(isNetworkError("appcast enclosure missing edSignature")).toBe(false);
    expect(isNetworkError("EdDSA signature does not match")).toBe(false);
  });
});

describe("settings API", () => {
  it("migrates legacy browser settings into startup and periodic checks", async () => {
    localStorage.setItem(
      "cam.settings",
      JSON.stringify({
        source: "mirror",
        customUrl: "",
        autoCheck: false,
        askBefore: true,
        signedOnly: true,
      }),
    );

    const settings = await managerApi.getSettings();

    expect(settings.source).toBe("mirror");
    expect(settings.autoCheck).toBe(false);
    expect(settings.checkOnStartup).toBe(false);
    expect(settings.periodicCheck).toBe(false);
    expect(settings.periodicCheckIntervalSeconds).toBe(15 * 60);
    expect(settings.disableCodexSelfUpdates).toBe(false);
  });

  it("normalizes and broadcasts browser settings writes", async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal("window", {
      open: vi.fn(),
      __TAURI_INTERNALS__: undefined,
      dispatchEvent,
    });

    const saved = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      periodicCheckIntervalSeconds: 0,
      disableCodexSelfUpdates: true,
    });

    expect(saved.periodicCheckIntervalSeconds).toBe(60);
    expect(saved.disableCodexSelfUpdates).toBe(true);
    expect(dispatchEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        type: SETTINGS_CHANGED_EVENT,
        detail: saved,
      }),
    );
  });

  it("coerces empty custom source and proxy modes to real defaults", async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal("window", {
      open: vi.fn(),
      __TAURI_INTERNALS__: undefined,
      dispatchEvent,
    });

    const saved = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      source: "custom",
      customUrl: "  ",
      proxyMode: "custom",
      customProxyUrl: "",
    });

    expect(saved.source).toBe("auto");
    expect(saved.customUrl).toBe("");
    expect(saved.proxyMode).toBe("system");
    expect(saved.customProxyUrl).toBe("");
  });

  it("keeps a supported codexLocale and drops unsupported values to null", async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal("window", {
      open: vi.fn(),
      __TAURI_INTERNALS__: undefined,
      dispatchEvent,
    });

    const zh = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      codexLocale: "zh-CN",
    });
    expect(zh.codexLocale).toBe("zh-CN");

    const bogus = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      codexLocale: "xx-YY" as unknown as string,
    });
    expect(bogus.codexLocale).toBeNull();

    const system = await managerApi.setSettings({
      ...DEFAULT_SETTINGS,
      codexLocale: null,
    });
    expect(system.codexLocale).toBeNull();
  });
});

describe("diagnostics API", () => {
  it("returns browser fallbacks without invoking Tauri", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const diagnostics = await managerApi.getDiagnostics();

    expect(diagnostics.os).toBe("browser");
    await expect(managerApi.openLogsDir()).resolves.toBeUndefined();
    await expect(managerApi.openCodexHome()).resolves.toBeUndefined();
    await expect(managerApi.frontendReady("en", 1, "browser-token")).resolves.toBeNull();
    await expect(
      managerApi.reportFrontendError({
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      }),
    ).resolves.toBeUndefined();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledWith(
      "[frontend]",
      expect.objectContaining({ kind: "test", message: "boom" }),
    );
    consoleError.mockRestore();
  });

  it("reports frontend readiness and application language through IPC", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue(null);

    await expect(managerApi.frontendReady("zh-TW", 7, "generation-token")).resolves.toBeNull();

    expect(invokeMock).toHaveBeenCalledWith("frontend_ready", {
      lang: "zh-TW",
      generation: 7,
      token: "generation-token",
    });
  });

  it("rejects malformed packaged smoke contexts at the IPC boundary", async () => {
    window.__TAURI_INTERNALS__ = {};

    invokeMock.mockResolvedValueOnce({ runId: "smoke-run_1", alignment: true });
    await expect(managerApi.frontendReady("en", 1, "token")).resolves.toEqual({
      runId: "smoke-run_1",
      alignment: true,
    });

    for (const malformed of [
      undefined,
      { runId: "", alignment: true },
      { runId: "../escape", alignment: true },
      { runId: "smoke-run", alignment: "true" },
      { runId: "smoke-run", alignment: false },
    ]) {
      invokeMock.mockResolvedValueOnce(malformed);
      await expect(managerApi.frontendReady("en", 1, "token")).rejects.toThrow(
        "invalid packaged smoke context",
      );
    }
  });

  it("records packaged alignment smoke results through IPC", async () => {
    window.__TAURI_INTERNALS__ = {};
    invokeMock.mockResolvedValue(undefined);

    await expect(
      managerApi.recordPackagedAlignmentSmokeResult("smoke-run", "failed", "restore failed"),
    ).resolves.toBeUndefined();

    expect(invokeMock).toHaveBeenCalledWith("record_packaged_alignment_smoke_result", {
      runId: "smoke-run",
      status: "failed",
      message: "restore failed",
    });
  });

  it("invokes diagnostics commands inside Tauri", async () => {
    window.__TAURI_INTERNALS__ = {};
    const diagnostics = {
      appVersion: "0.1.17",
      os: "macos",
      arch: "aarch64",
      locale: null,
      updateSource: "auto",
      customSourceHost: null,
      windowsInstallMode: null,
      installStatus: "macos status=none",
      configHealth: {
        settingsStatus: "ok",
        provenanceStatus: "ok",
        unknownSource: null,
        detail: null,
        settingsBackupAvailable: false,
        provenanceBackupAvailable: false,
      },
      logsDir: "/tmp/logs",
      recentErrors: [],
      logTail: "",
      generatedAtUnix: 1,
    };
    invokeMock
      .mockResolvedValueOnce(diagnostics)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(undefined);

    await expect(managerApi.getDiagnostics()).resolves.toEqual(diagnostics);
    await expect(managerApi.openLogsDir()).resolves.toBeUndefined();
    await expect(managerApi.openCodexHome()).resolves.toBeUndefined();
    await expect(
      managerApi.reportFrontendError({
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      }),
    ).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenNthCalledWith(1, "get_diagnostics");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "open_logs_dir");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "open_codex_home");
    expect(invokeMock).toHaveBeenNthCalledWith(4, "log_frontend_error", {
      payload: {
        kind: "test",
        message: "boom",
        stack: null,
        componentStack: null,
      },
    });
  });
});
