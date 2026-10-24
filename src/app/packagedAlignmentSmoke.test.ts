import { describe, expect, it, vi } from "vitest";

import type { AlignmentReport, AlignmentScan } from "../shared/types";
import { runPackagedAlignmentSmoke } from "./packagedAlignmentSmoke";

const SCAN: AlignmentScan = {
  scanToken: "smoke-scan",
  activeProvider: "openai",
  databaseCount: 1,
  threadCount: 1,
  mismatchedThreads: 1,
  rolloutCount: 2,
  mismatchedRollouts: 1,
  skippedRollouts: 1,
  skipReasons: ["rollout-invalid.jsonl: invalid session metadata"],
  desktopRunning: false,
  cliRunning: false,
  canExecute: true,
  warnings: [],
};

const COMMITTED: AlignmentReport = {
  outcome: "committed",
  recoveryId: "smoke-recovery",
  backupDir: "smoke/recovery-backups/smoke-recovery",
  threadsUpdated: 1,
  rolloutsUpdated: 1,
  rolloutsSkipped: 1,
  writeVerified: true,
  rollbackVerified: false,
  codexWasRunning: false,
  restartAllowed: false,
  undoAllowed: true,
  elevationRequired: false,
  errorCode: null,
};

describe("packaged alignment smoke", () => {
  it("uses the public IPC workflow and requires verified commit and restore reports", async () => {
    const api = {
      toolboxAlignmentScan: vi.fn().mockResolvedValue(SCAN),
      toolboxAlignmentExecute: vi.fn().mockResolvedValue(COMMITTED),
      toolboxAlignmentRestore: vi.fn().mockResolvedValue({
        ...COMMITTED,
        outcome: "restored",
        threadsUpdated: 0,
        rolloutsUpdated: 0,
        writeVerified: false,
        rollbackVerified: true,
        undoAllowed: false,
      }),
      recordPackagedAlignmentSmokeResult: vi.fn().mockResolvedValue(undefined),
    };

    await runPackagedAlignmentSmoke({ runId: "smoke-1", alignment: true }, api);

    expect(api.toolboxAlignmentScan).toHaveBeenCalledOnce();
    expect(api.toolboxAlignmentExecute).toHaveBeenCalledWith("smoke-scan");
    expect(api.toolboxAlignmentRestore).toHaveBeenCalledWith("smoke-recovery");
    expect(api.recordPackagedAlignmentSmokeResult).toHaveBeenCalledWith(
      "smoke-1",
      "passed",
      null,
    );
  });

  it("persists an observable failure before rejecting the smoke run", async () => {
    const api = {
      toolboxAlignmentScan: vi.fn().mockResolvedValue(SCAN),
      toolboxAlignmentExecute: vi.fn().mockRejectedValue(new Error("execute failed")),
      toolboxAlignmentRestore: vi.fn(),
      recordPackagedAlignmentSmokeResult: vi.fn().mockResolvedValue(undefined),
    };

    await expect(
      runPackagedAlignmentSmoke({ runId: "smoke-2", alignment: true }, api),
    ).rejects.toThrow("execute failed");

    expect(api.recordPackagedAlignmentSmokeResult).toHaveBeenCalledWith(
      "smoke-2",
      "failed",
      "execute failed",
    );
  });

  it("rejects a commit that does not update one rollout and skip one invalid rollout", async () => {
    const api = {
      toolboxAlignmentScan: vi.fn().mockResolvedValue(SCAN),
      toolboxAlignmentExecute: vi.fn().mockResolvedValue({
        ...COMMITTED,
        rolloutsSkipped: 0,
      }),
      toolboxAlignmentRestore: vi.fn(),
      recordPackagedAlignmentSmokeResult: vi.fn().mockResolvedValue(undefined),
    };

    await expect(
      runPackagedAlignmentSmoke({ runId: "smoke-3", alignment: true }, api),
    ).rejects.toThrow("expected one skipped rollout");

    expect(api.toolboxAlignmentRestore).not.toHaveBeenCalled();
    expect(api.recordPackagedAlignmentSmokeResult).toHaveBeenCalledWith(
      "smoke-3",
      "failed",
      expect.stringContaining("expected one skipped rollout"),
    );
  });
});
