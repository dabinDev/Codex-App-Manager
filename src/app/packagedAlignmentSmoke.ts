import {
  errorMessage,
  managerApi,
  type PackagedSmokeContext,
} from "../services/managerApi";

type AlignmentSmokeApi = Pick<
  typeof managerApi,
  | "toolboxAlignmentScan"
  | "toolboxAlignmentExecute"
  | "toolboxAlignmentRestore"
  | "recordPackagedAlignmentSmokeResult"
>;

function requireSmoke(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(`packaged alignment smoke: ${message}`);
}

export async function runPackagedAlignmentSmoke(
  context: PackagedSmokeContext,
  api: AlignmentSmokeApi = managerApi,
): Promise<void> {
  if (!context.alignment) return;

  try {
    const scan = await api.toolboxAlignmentScan();
    requireSmoke(scan.canExecute, "scan was blocked");
    requireSmoke(scan.databaseCount >= 1, "database fixture was not discovered");
    requireSmoke(scan.mismatchedThreads >= 1, "database fixture was already aligned");
    requireSmoke(scan.mismatchedRollouts >= 1, "rollout fixture was already aligned");
    requireSmoke(scan.skippedRollouts === 1, "expected one skipped rollout during scan");

    const committed = await api.toolboxAlignmentExecute(scan.scanToken);
    requireSmoke(committed.outcome === "committed", `unexpected outcome ${committed.outcome}`);
    requireSmoke(committed.rolloutsUpdated === 1, "expected one updated rollout");
    requireSmoke(committed.rolloutsSkipped === 1, "expected one skipped rollout");
    requireSmoke(committed.writeVerified, "commit was not verified");
    requireSmoke(committed.undoAllowed, "verified recovery was not available");
    requireSmoke(Boolean(committed.recoveryId), "recovery id was missing");

    const restored = await api.toolboxAlignmentRestore(committed.recoveryId as string);
    requireSmoke(restored.outcome === "restored", `unexpected restore ${restored.outcome}`);
    requireSmoke(restored.rollbackVerified, "restore was not verified");

    await api.recordPackagedAlignmentSmokeResult(context.runId, "passed", null);
  } catch (cause) {
    const message = errorMessage(cause);
    try {
      await api.recordPackagedAlignmentSmokeResult(context.runId, "failed", message);
    } catch (statusCause) {
      throw new Error(
        `${message}; failed to persist smoke failure: ${errorMessage(statusCause)}`,
      );
    }
    throw cause;
  }
}
