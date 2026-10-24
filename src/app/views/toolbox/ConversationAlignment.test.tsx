import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../../services/managerApi";
import type { AlignmentReport, AlignmentScan } from "../../../shared/types";
import { I18nProvider } from "../../i18n";
import { ConversationAlignment } from "./ConversationAlignment";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const listenMock = vi.mocked(listen);
type ElevationPayload = { scanToken: string; operation: string };
let onElevation: ((event: { payload: ElevationPayload }) => void) | undefined;

const READY_SCAN: AlignmentScan = {
  scanToken: "scan-token",
  activeProvider: "openai",
  databaseCount: 2,
  threadCount: 24,
  mismatchedThreads: 3,
  rolloutCount: 8,
  mismatchedRollouts: 2,
  skippedRollouts: 0,
  skipReasons: [],
  desktopRunning: true,
  cliRunning: false,
  canExecute: true,
  warnings: [],
};

const COMMITTED_REPORT: AlignmentReport = {
  outcome: "committed",
  recoveryId: "provider-align-1",
  backupDir: "C:/tmp/provider-align-1",
  threadsUpdated: 3,
  rolloutsUpdated: 2,
  rolloutsSkipped: 0,
  writeVerified: true,
  rollbackVerified: false,
  codexWasRunning: true,
  restartAllowed: true,
  undoAllowed: true,
  elevationRequired: false,
  errorCode: null,
};

function renderAlignment() {
  return render(
    <I18nProvider>
      <ConversationAlignment />
    </I18nProvider>,
  );
}

describe("Conversation alignment workflow", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    vi.restoreAllMocks();
    onElevation = undefined;
    listenMock.mockReset();
    listenMock.mockImplementation(async (_event, handler) => {
      onElevation = handler as (event: { payload: ElevationPayload }) => void;
      return () => {
        onElevation = undefined;
      };
    });
    vi.spyOn(managerApi, "toolboxAlignmentScan").mockResolvedValue(READY_SCAN);
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockResolvedValue(COMMITTED_REPORT);
    vi.spyOn(managerApi, "toolboxAlignmentRestore").mockResolvedValue({
      ...COMMITTED_REPORT,
      outcome: "restored",
      threadsUpdated: 0,
      rolloutsUpdated: 0,
      writeVerified: false,
      rollbackVerified: true,
      restartAllowed: true,
      undoAllowed: false,
    });
    vi.spyOn(managerApi, "toolboxAlignmentOpenBackup").mockResolvedValue(undefined);
    vi.spyOn(managerApi, "toolboxAlignmentRestart").mockResolvedValue(undefined);
    vi.spyOn(managerApi, "toolboxForceRestartCodex").mockResolvedValue(undefined);
  });

  it("scans automatically and presents the exact impact", async () => {
    renderAlignment();

    expect(screen.getByRole("status")).toHaveTextContent("Scanning local metadata");
    expect(await screen.findByText("3 conversations need alignment")).toBeInTheDocument();
    expect(screen.getByText("2 session files need alignment")).toBeInTheDocument();
    expect(screen.getByText("openai")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start alignment" })).toBeEnabled();
  });

  it("reports an active Codex CLI without blocking safe alignment", async () => {
    vi.spyOn(managerApi, "toolboxAlignmentScan").mockResolvedValue({
      ...READY_SCAN,
      cliRunning: true,
      canExecute: true,
      warnings: ["alignment_cli_running"],
    });

    renderAlignment();

    expect(await screen.findByText("A Codex CLI session is running.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start alignment" })).toBeEnabled();
  });

  it("keeps alignment enabled and reports malformed session files separately", async () => {
    vi.spyOn(managerApi, "toolboxAlignmentScan").mockResolvedValue({
      ...READY_SCAN,
      skippedRollouts: 1,
      skipReasons: ["alignment_rollout_invalid_json"],
    });

    renderAlignment();

    expect(await screen.findByText("1 session file will be skipped")).toBeInTheDocument();
    expect(screen.getByText("Invalid JSON metadata")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start alignment" })).toBeEnabled();
  });

  it("uses plural skipped-file copy for multiple malformed files", async () => {
    vi.spyOn(managerApi, "toolboxAlignmentScan").mockResolvedValue({
      ...READY_SCAN,
      skippedRollouts: 2,
      skipReasons: ["alignment_rollout_invalid_json"],
    });

    renderAlignment();

    expect(await screen.findByText("2 session files will be skipped")).toBeInTheDocument();
  });

  it("explains recovery upgrades skipped by the safe memory limit", async () => {
    vi.spyOn(managerApi, "toolboxAlignmentScan").mockResolvedValue({
      ...READY_SCAN,
      skippedRollouts: 1,
      skipReasons: ["alignment_recovery_artifact_budget"],
    });

    renderAlignment();

    expect(
      await screen.findByText("Recovery upgrade exceeds safe memory limit"),
    ).toBeInTheDocument();
  });

  it("shows the UAC waiting state only for the active execution", async () => {
    const user = userEvent.setup();
    let resolveExecute: ((report: AlignmentReport) => void) | undefined;
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockImplementation(
      () => new Promise<AlignmentReport>((resolve) => (resolveExecute = resolve)),
    );

    renderAlignment();
    await waitFor(() => {
      expect(listenMock).toHaveBeenCalledWith(
        "toolbox://alignment-elevation",
        expect.any(Function),
      );
    });
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    act(() => {
      onElevation?.({ payload: { scanToken: "old-token", operation: "align" } });
    });
    expect(screen.queryByText("Waiting for administrator approval")).not.toBeInTheDocument();

    act(() => {
      onElevation?.({ payload: { scanToken: "scan-token", operation: "align" } });
    });
    expect(await screen.findByText("Waiting for administrator approval")).toBeInTheDocument();

    await act(async () => {
      resolveExecute?.(COMMITTED_REPORT);
    });
    expect(await screen.findByText("Alignment completed and verified.")).toBeInTheDocument();
  });

  it("ignores elevation events after the active execution settles", async () => {
    const user = userEvent.setup();
    renderAlignment();
    await waitFor(() => expect(onElevation).toBeDefined());
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));
    await screen.findByText("Alignment completed and verified.");

    act(() => {
      onElevation?.({ payload: { scanToken: "scan-token", operation: "align" } });
    });

    expect(screen.queryByText("Waiting for administrator approval")).not.toBeInTheDocument();
    expect(screen.getByText("Alignment completed and verified.")).toBeInTheDocument();
  });

  it("unsubscribes from elevation events on unmount", async () => {
    const stopListening = vi.fn();
    listenMock.mockResolvedValue(stopListening);
    const view = renderAlignment();
    await waitFor(() => expect(listenMock).toHaveBeenCalled());

    view.unmount();

    expect(stopListening).toHaveBeenCalledTimes(1);
  });

  it("reports a cancelled UAC prompt without losing the scan context", async () => {
    const user = userEvent.setup();
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockRejectedValue({
      code: "alignment_elevation_cancelled",
      message: "cancelled",
    });

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(await screen.findByText("Administrator approval was cancelled.")).toBeInTheDocument();
    expect(screen.getByText("openai")).toBeInTheDocument();
  });

  it("keeps the scan and explains when CLI starts after confirmation", async () => {
    const user = userEvent.setup();
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockRejectedValue({
      code: "alignment_cli_running",
      message: "CLI started after confirmation",
    });

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(await screen.findByText("A Codex CLI session is running.")).toBeInTheDocument();
    expect(screen.getByText("openai")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Scan again" })).toBeEnabled();
  });

  it("shows the structured CLI failure and restart action after Codex was stopped", async () => {
    const user = userEvent.setup();
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockResolvedValue({
      ...COMMITTED_REPORT,
      outcome: "stale_plan",
      recoveryId: null,
      backupDir: null,
      threadsUpdated: 0,
      rolloutsUpdated: 0,
      writeVerified: false,
      restartAllowed: true,
      undoAllowed: false,
      errorCode: "alignment_cli_running",
    });

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(await screen.findByText("A Codex CLI session is running.")).toBeInTheDocument();
    expect(screen.getByText("openai")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart Codex" })).toBeEnabled();
  });

  it("requires confirmation and exposes verified recovery actions", async () => {
    const user = userEvent.setup();
    renderAlignment();

    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    const dialog = screen.getByRole("dialog", { name: "Confirm alignment" });
    expect(dialog).toHaveTextContent("3 conversations and 2 session files");
    expect(dialog).toHaveTextContent("Codex may close before local files change");

    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(managerApi.toolboxAlignmentExecute).toHaveBeenCalledWith("scan-token");
    expect(await screen.findByText("Alignment completed and verified.")).toBeInTheDocument();
    expect(screen.getByText("Restart required before aligned conversations appear.")).toBeInTheDocument();
    expect(screen.getByText("C:/tmp/provider-align-1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open backup folder" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Undo alignment" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Restart Codex" })).toBeEnabled();
  });

  it("reports multiple skipped files alongside a verified commit", async () => {
    const user = userEvent.setup();
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockResolvedValue({
      ...COMMITTED_REPORT,
      rolloutsSkipped: 2,
    });

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(
      await screen.findByText(
        "Alignment completed while leaving 2 unsafe session files unchanged.",
      ),
    ).toBeInTheDocument();
  });

  it("rescans after a stale plan", async () => {
    const user = userEvent.setup();
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockResolvedValue({
      ...COMMITTED_REPORT,
      outcome: "stale_plan",
      recoveryId: null,
      backupDir: null,
      threadsUpdated: 0,
      rolloutsUpdated: 0,
      writeVerified: false,
      codexWasRunning: false,
      restartAllowed: false,
      undoAllowed: false,
    });
    const scanSpy = vi.spyOn(managerApi, "toolboxAlignmentScan");

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(await screen.findByText("Local metadata changed after the scan.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Scan again" }));
    expect(scanSpy).toHaveBeenCalledTimes(2);
  });

  it("restores from the verified backup and keeps restart gating from the report", async () => {
    const user = userEvent.setup();
    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));
    await screen.findByText("Alignment completed and verified.");

    await user.click(screen.getByRole("button", { name: "Undo alignment" }));

    expect(managerApi.toolboxAlignmentRestore).toHaveBeenCalledWith("provider-align-1");
    expect(await screen.findByText("The original metadata was restored and verified.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Undo alignment" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart Codex" })).toBeEnabled();
  });

  it("shows recovery guidance without restart when automatic recovery is incomplete", async () => {
    const user = userEvent.setup();
    vi.spyOn(managerApi, "toolboxAlignmentExecute").mockResolvedValue({
      ...COMMITTED_REPORT,
      outcome: "recovery_required",
      writeVerified: false,
      rollbackVerified: false,
      restartAllowed: false,
      undoAllowed: false,
      errorCode: "alignment_mutation_failed",
    });

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Start alignment" }));
    await user.click(screen.getByRole("button", { name: "Back up and align" }));

    expect(await screen.findByText("Automatic recovery could not be verified.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open backup folder" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Restart Codex" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Force restart Codex" })).toBeEnabled();
  });

  it("confirms and deduplicates manual force restart independently", async () => {
    const user = userEvent.setup();
    let resolveRestart: (() => void) | undefined;
    const forceRestart = vi.spyOn(managerApi, "toolboxForceRestartCodex").mockImplementation(
      () => new Promise<void>((resolve) => (resolveRestart = resolve)),
    );

    renderAlignment();
    await user.click(await screen.findByRole("button", { name: "Force restart Codex" }));
    expect(forceRestart).not.toHaveBeenCalled();
    const restartNow = screen.getByRole("button", { name: "Restart now" });
    await user.dblClick(restartNow);

    expect(forceRestart).toHaveBeenCalledTimes(1);
    expect(forceRestart).toHaveBeenCalledWith(true);
    await act(async () => {
      resolveRestart?.();
    });
    expect(await screen.findByText("Codex was force restarted.")).toBeInTheDocument();
  });
});
