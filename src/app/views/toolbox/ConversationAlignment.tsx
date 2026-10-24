import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { errorCode, managerApi } from "../../../services/managerApi";
import type { AlignmentReport, AlignmentScan } from "../../../shared/types";
import { Ring, StatusBanner } from "../../components";
import { Icon } from "../../icons";
import { useI18n, type TKey } from "../../i18n";
import { Sheet } from "../../Sheet";

type ResultAction =
  | "idle"
  | "opening-backup"
  | "restoring"
  | "restarting"
  | "force-restarting";

type ElevationPayload = {
  scanToken: string;
  operation: "align" | "restore_then_align";
};

type ForceRestartStatus =
  | { kind: "idle" }
  | { kind: "restarting" }
  | { kind: "complete" }
  | { kind: "error"; code: string };

type AlignmentUiState =
  | { kind: "scanning" }
  | { kind: "ready"; scan: AlignmentScan }
  | { kind: "confirming"; scan: AlignmentScan }
  | { kind: "executing"; scan: AlignmentScan; awaitingElevation: boolean }
  | {
      kind: "result";
      scan: AlignmentScan;
      report: AlignmentReport;
      action: ResultAction;
      notice?: "backup-opened" | "restart-complete";
      actionError?: string;
      restartConsumed?: boolean;
    }
  | { kind: "error"; code: string; scan?: AlignmentScan };

const RESULT_COPY: Record<AlignmentReport["outcome"], TKey> = {
  committed: "toolbox.alignment.committed",
  no_changes: "toolbox.alignment.noChangesResult",
  stale_plan: "toolbox.alignment.stale",
  restored: "toolbox.alignment.restored",
  recovery_required: "toolbox.alignment.recoveryRequired",
};

function errorCopy(code: string): TKey {
  switch (code) {
    case "desktop_required":
    case "alignment_codex_home_unavailable":
    case "alignment_config_unavailable":
    case "alignment_provider_missing":
      return "toolbox.alignment.error.unavailable";
    case "alignment_config_invalid":
    case "alignment_unsupported_schema":
    case "alignment_blocked_metadata":
    case "alignment_rollout_head_too_large":
    case "alignment_rollout_invalid_json":
    case "alignment_rollout_missing_session_meta":
    case "alignment_rollout_provider_missing":
      return "toolbox.alignment.error.invalidMetadata";
    case "alignment_unsafe_path":
      return "toolbox.alignment.error.unsafePath";
    case "alignment_busy":
    case "provider_busy":
    case "operation_busy":
      return "toolbox.alignment.error.busy";
    case "alignment_cli_running":
      return "toolbox.alignment.cliBlocked";
    case "alignment_stale_recovery":
    case "alignment_invalid_recovery":
    case "alignment_backup_failed":
      return "toolbox.alignment.error.recovery";
    case "alignment_restart_not_allowed":
      return "toolbox.alignment.error.restart";
    case "alignment_elevation_cancelled":
      return "toolbox.alignment.elevationCancelled";
    case "alignment_permission_denied":
      return "toolbox.alignment.permissionDenied";
    case "alignment_elevation_invalid_request":
    case "alignment_elevation_expired":
    case "alignment_elevation_replayed":
    case "alignment_elevation_io":
    case "alignment_elevation_invalid_result":
    case "alignment_elevation_failed":
    case "alignment_elevation_unsupported":
    case "alignment_elevation_timeout":
    case "alignment_elevation_launch_failed":
      return "toolbox.alignment.elevationFailed";
    case "contract_mismatch":
      return "toolbox.alignment.error.contract";
    case "alignment_io":
    case "alignment_database":
    case "alignment_mutation_failed":
    default:
      return "toolbox.alignment.error.unknown";
  }
}

function skipReasonCopy(code: string): TKey {
  switch (code) {
    case "alignment_rollout_invalid_json":
      return "toolbox.alignment.skipReason.invalidJson";
    case "alignment_rollout_head_too_large":
      return "toolbox.alignment.skipReason.oversized";
    case "alignment_recovery_artifact_budget":
      return "toolbox.alignment.skipReason.recoveryBudget";
    case "alignment_rollout_provider_invalid_type":
      return "toolbox.alignment.skipReason.invalidProvider";
    case "alignment_rollout_missing_session_meta":
    case "alignment_rollout_payload_missing":
    default:
      return "toolbox.alignment.skipReason.invalidStructure";
  }
}

function warningCopy(code: string): TKey {
  if (code === "alignment_cli_running") return "toolbox.alignment.cliBlocked";
  if (code.startsWith("alignment_rollout_")) {
    return "toolbox.alignment.metadataBlocked";
  }
  return errorCopy(code);
}

function reportTone(outcome: AlignmentReport["outcome"]): "err" | "info" | "ok" | "warn" {
  if (outcome === "recovery_required") return "err";
  if (outcome === "restored" || outcome === "stale_plan") return "warn";
  if (outcome === "committed") return "ok";
  return "info";
}

function stateScan(state: AlignmentUiState): AlignmentScan | null {
  return "scan" in state ? (state.scan ?? null) : null;
}

export function ConversationAlignment() {
  const { t } = useI18n();
  const [state, setState] = useState<AlignmentUiState>({ kind: "scanning" });
  const [forceRestartOpen, setForceRestartOpen] = useState(false);
  const [forceRestartStatus, setForceRestartStatus] = useState<ForceRestartStatus>({
    kind: "idle",
  });
  const requestSequence = useRef(0);
  const executionInFlight = useRef(false);
  const activeExecutionToken = useRef<string | null>(null);
  const forceRestartInFlight = useRef(false);

  const scan = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setState({ kind: "scanning" });
    try {
      const next = await managerApi.toolboxAlignmentScan();
      if (requestSequence.current === sequence) setState({ kind: "ready", scan: next });
    } catch (cause) {
      if (requestSequence.current === sequence) {
        setState({ kind: "error", code: errorCode(cause) ?? "unknown" });
      }
    }
  }, []);

  useEffect(() => {
    void scan();
    return () => {
      requestSequence.current += 1;
    };
  }, [scan]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<ElevationPayload>("toolbox://alignment-elevation", (event) => {
      if (disposed || event.payload.scanToken !== activeExecutionToken.current) return;
      setState((current) =>
        current.kind === "executing" &&
        current.scan.scanToken === event.payload.scanToken
          ? { ...current, awaitingElevation: true }
          : current,
      );
    }).then((stopListening) => {
      if (disposed) stopListening();
      else unlisten = stopListening;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const execute = async () => {
    if (state.kind !== "confirming" || executionInFlight.current) return;
    const { scan: confirmedScan } = state;
    executionInFlight.current = true;
    activeExecutionToken.current = confirmedScan.scanToken;
    setState({ kind: "executing", scan: confirmedScan, awaitingElevation: false });
    try {
      const report = await managerApi.toolboxAlignmentExecute(confirmedScan.scanToken);
      setState({ kind: "result", scan: confirmedScan, report, action: "idle" });
    } catch (cause) {
      setState({
        kind: "error",
        code: errorCode(cause) ?? "unknown",
        scan: confirmedScan,
      });
    } finally {
      activeExecutionToken.current = null;
      executionInFlight.current = false;
    }
  };

  const forceRestart = async () => {
    if (forceRestartInFlight.current) return;
    forceRestartInFlight.current = true;
    setForceRestartOpen(false);
    setForceRestartStatus({ kind: "restarting" });
    try {
      await managerApi.toolboxForceRestartCodex(true);
      setForceRestartStatus({ kind: "complete" });
    } catch (cause) {
      setForceRestartStatus({
        kind: "error",
        code: errorCode(cause) ?? "unknown",
      });
    } finally {
      forceRestartInFlight.current = false;
    }
  };

  const openBackup = async () => {
    if (state.kind !== "result") return;
    const recoveryId = state.report.recoveryId;
    if (!recoveryId) return;
    const current = state;
    setState({ ...current, action: "opening-backup", actionError: undefined });
    try {
      await managerApi.toolboxAlignmentOpenBackup(recoveryId);
      setState({ ...current, action: "idle", notice: "backup-opened" });
    } catch (cause) {
      setState({
        ...current,
        action: "idle",
        actionError: errorCode(cause) ?? "unknown",
      });
    }
  };

  const restore = async () => {
    if (state.kind !== "result" || !state.report.undoAllowed) return;
    const recoveryId = state.report.recoveryId;
    if (!recoveryId) return;
    const current = state;
    setState({ ...current, action: "restoring", actionError: undefined });
    try {
      const report = await managerApi.toolboxAlignmentRestore(recoveryId);
      setState({ kind: "result", scan: current.scan, report, action: "idle" });
    } catch (cause) {
      setState({
        ...current,
        action: "idle",
        actionError: errorCode(cause) ?? "unknown",
      });
    }
  };

  const restart = async () => {
    if (state.kind !== "result" || !state.report.restartAllowed || state.restartConsumed) return;
    const current = state;
    setState({ ...current, action: "restarting", actionError: undefined });
    try {
      await managerApi.toolboxAlignmentRestart();
      setState({
        ...current,
        action: "idle",
        notice: "restart-complete",
        restartConsumed: true,
      });
    } catch (cause) {
      setState({
        ...current,
        action: "idle",
        actionError: errorCode(cause) ?? "unknown",
      });
    }
  };

  const activeScan = stateScan(state);
  const mismatchCount = activeScan
    ? activeScan.mismatchedThreads + activeScan.mismatchedRollouts
    : 0;
  const canStart =
    state.kind === "ready" && state.scan.canExecute && mismatchCount > 0;
  const confirmationOpen = state.kind === "confirming";
  const anyConfirmationOpen = confirmationOpen || forceRestartOpen;
  const protectedActionBusy =
    state.kind === "executing" ||
    (state.kind === "result" && state.action !== "idle") ||
    forceRestartStatus.kind === "restarting";

  return (
    <>
      <section
        className="alignment-tool"
        aria-labelledby="alignment-title"
        inert={anyConfirmationOpen ? true : undefined}
      >
        <header className="alignment-heading">
          <span className="alignment-mark" aria-hidden="true">
            <Icon name="message" />
          </span>
          <div>
            <h1 id="alignment-title">{t("toolbox.alignment.title")}</h1>
            <p>{t("toolbox.alignment.intro")}</p>
          </div>
        </header>

        <div className="alignment-privacy">
          <Icon name="shield" />
          <span>{t("toolbox.alignment.localOnly")}</span>
        </div>

        {activeScan ? (
          <dl className="alignment-metrics">
            <div>
              <dt>{t("toolbox.alignment.provider")}</dt>
              <dd title={activeScan.activeProvider}>{activeScan.activeProvider}</dd>
            </div>
            <div>
              <dt>{t("toolbox.alignment.databases")}</dt>
              <dd>{activeScan.databaseCount}</dd>
            </div>
            <div>
              <dt>{t("toolbox.alignment.conversations")}</dt>
              <dd>
                {activeScan.mismatchedThreads} / {activeScan.threadCount}
              </dd>
            </div>
            <div>
              <dt>{t("toolbox.alignment.sessionFiles")}</dt>
              <dd>
                {activeScan.mismatchedRollouts} / {activeScan.rolloutCount}
              </dd>
            </div>
            <div>
              <dt>{t("toolbox.alignment.skipped")}</dt>
              <dd>{activeScan.skippedRollouts}</dd>
            </div>
          </dl>
        ) : null}

        {activeScan ? (
          <div className="alignment-impact" aria-live="polite">
            <strong>
              {t("toolbox.alignment.threadImpact", {
                count: activeScan.mismatchedThreads,
              })}
            </strong>
            <span>
              {t("toolbox.alignment.rolloutImpact", {
                count: activeScan.mismatchedRollouts,
              })}
            </span>
          </div>
        ) : null}

        {state.kind === "scanning" ? (
          <StatusBanner tone="info" icon="loader">
            {t("toolbox.alignment.scanning")}
          </StatusBanner>
        ) : null}

        {state.kind === "error" ? (
          <StatusBanner
            tone="err"
            action={
              <button type="button" className="linkbtn" onClick={() => void scan()}>
                {t("toolbox.alignment.scanAgain")}
              </button>
            }
          >
            {t(errorCopy(state.code))}
          </StatusBanner>
        ) : null}

        {state.kind === "ready"
          ? state.scan.warnings.map((warning) => (
              <StatusBanner key={warning} tone="warn">
                {t(warningCopy(warning))}
              </StatusBanner>
            ))
          : null}

        {activeScan && activeScan.skippedRollouts > 0 ? (
          <StatusBanner tone="warn">
            <div className="alignment-skip-notice">
              <strong>
                {t(
                  activeScan.skippedRollouts === 1
                    ? "toolbox.alignment.skippedImpact"
                    : "toolbox.alignment.skippedImpactMany",
                  {
                  count: activeScan.skippedRollouts,
                  },
                )}
              </strong>
              {activeScan.skipReasons.map((reason) => (
                <span key={reason}>{t(skipReasonCopy(reason))}</span>
              ))}
            </div>
          </StatusBanner>
        ) : null}

        {state.kind === "ready" && !state.scan.canExecute && state.scan.warnings.length === 0 ? (
          <StatusBanner tone="warn">{t("toolbox.alignment.metadataBlocked")}</StatusBanner>
        ) : null}

        {state.kind === "ready" && mismatchCount === 0 ? (
          <StatusBanner tone="ok">{t("toolbox.alignment.noChanges")}</StatusBanner>
        ) : null}

        {state.kind === "executing" ? (
          <StatusBanner tone="info" icon="loader">
            {t(
              state.awaitingElevation
                ? "toolbox.alignment.awaitingElevation"
                : "toolbox.alignment.running",
            )}
          </StatusBanner>
        ) : null}

        {state.kind === "result" ? (
          <div className="alignment-result">
            <StatusBanner tone={reportTone(state.report.outcome)}>
              {t(
                state.report.outcome === "stale_plan" && state.report.errorCode
                  ? errorCopy(state.report.errorCode)
                  : RESULT_COPY[state.report.outcome],
              )}
            </StatusBanner>

            {state.report.rolloutsSkipped > 0 ? (
              <StatusBanner tone="warn">
                {t(
                  state.report.rolloutsSkipped === 1
                    ? "toolbox.alignment.skippedResult"
                    : "toolbox.alignment.skippedResultMany",
                  { count: state.report.rolloutsSkipped },
                )}
              </StatusBanner>
            ) : null}

            {state.report.outcome === "committed" && state.report.restartAllowed ? (
              <StatusBanner tone="info">{t("toolbox.alignment.restartRequired")}</StatusBanner>
            ) : null}

            {state.report.backupDir ? (
              <div className="alignment-backup">
                <span>{t("config.backupPath")}</span>
                <code title={state.report.backupDir}>{state.report.backupDir}</code>
              </div>
            ) : null}

            {state.notice === "backup-opened" ? (
              <StatusBanner tone="info">{t("toolbox.alignment.backupOpened")}</StatusBanner>
            ) : null}
            {state.notice === "restart-complete" ? (
              <StatusBanner tone="ok">{t("toolbox.alignment.restartDone")}</StatusBanner>
            ) : null}
            {state.actionError ? (
              <StatusBanner tone="err">{t(errorCopy(state.actionError))}</StatusBanner>
            ) : null}

            <div className="alignment-result-actions">
              {state.report.recoveryId && state.report.backupDir ? (
                <button
                  type="button"
                  className="btn ghost"
                  disabled={state.action !== "idle"}
                  onClick={() => void openBackup()}
                >
                  <Icon name={state.action === "opening-backup" ? "loader" : "folder"} />
                  {state.action === "opening-backup"
                    ? t("toolbox.alignment.openingBackup")
                    : t("toolbox.alignment.openBackup")}
                </button>
              ) : null}
              {state.report.undoAllowed && state.report.recoveryId ? (
                <button
                  type="button"
                  className="btn ghost"
                  disabled={state.action !== "idle"}
                  onClick={() => void restore()}
                >
                  <Icon name={state.action === "restoring" ? "loader" : "refresh"} />
                  {state.action === "restoring"
                    ? t("toolbox.alignment.undoing")
                    : t("toolbox.alignment.undo")}
                </button>
              ) : null}
              {state.report.restartAllowed && !state.restartConsumed ? (
                <button
                  type="button"
                  className="btn primary"
                  disabled={state.action !== "idle"}
                  onClick={() => void restart()}
                >
                  <Icon name={state.action === "restarting" ? "loader" : "play"} />
                  {state.action === "restarting" ? t("config.restarting") : t("config.restart")}
                </button>
              ) : null}
              <button
                type="button"
                className="btn ghost"
                disabled={state.action !== "idle"}
                onClick={() => void scan()}
              >
                <Icon name="refresh" />
                {t("toolbox.alignment.scanAgain")}
              </button>
            </div>
          </div>
        ) : null}

        {state.kind === "ready" ? (
          <div className="alignment-actions">
            <button
              type="button"
              className="btn primary big"
              disabled={!canStart}
              onClick={() => setState({ kind: "confirming", scan: state.scan })}
            >
              <Icon name="play" />
              {t("toolbox.alignment.start")}
            </button>
            <button type="button" className="btn ghost" onClick={() => void scan()}>
              <Icon name="refresh" />
              {t("toolbox.alignment.scanAgain")}
            </button>
          </div>
        ) : null}

        <div className="alignment-force-restart">
          <button
            type="button"
            className="btn ghost"
            disabled={protectedActionBusy}
            onClick={() => {
              setForceRestartStatus({ kind: "idle" });
              setForceRestartOpen(true);
            }}
          >
            <Icon name={forceRestartStatus.kind === "restarting" ? "loader" : "refresh"} />
            {forceRestartStatus.kind === "restarting"
              ? t("toolbox.alignment.forceRestarting")
              : t("toolbox.alignment.forceRestart")}
          </button>
          {forceRestartStatus.kind === "complete" ? (
            <StatusBanner tone="ok">{t("toolbox.alignment.forceRestartDone")}</StatusBanner>
          ) : null}
          {forceRestartStatus.kind === "error" ? (
            <StatusBanner tone="err">
              {forceRestartStatus.code === "alignment_busy"
                ? t("toolbox.alignment.error.busy")
                : t("toolbox.alignment.forceRestartFailed")}
            </StatusBanner>
          ) : null}
        </div>
      </section>

      <Sheet
        open={confirmationOpen}
        onDismiss={() => {
          if (state.kind === "confirming") setState({ kind: "ready", scan: state.scan });
        }}
        labelledBy="alignment-confirm-title"
        describedBy="alignment-confirm-body"
        initialFocus="dismiss"
        centeredInExpanded
      >
        <Ring icon="shield" variant="accent" />
        <h3 id="alignment-confirm-title">{t("toolbox.alignment.confirm.title")}</h3>
        <p id="alignment-confirm-body">
          {t("toolbox.alignment.confirm.body", {
            threads: activeScan?.mismatchedThreads ?? 0,
            rollouts: activeScan?.mismatchedRollouts ?? 0,
          })}
        </p>
        <p>{t("toolbox.alignment.confirm.closeWarning")}</p>
        <div className="row2 sheet-actions">
          <button
            type="button"
            className="btn ghost"
            onClick={() => {
              if (state.kind === "confirming") setState({ kind: "ready", scan: state.scan });
            }}
          >
            {t("config.confirm.cancel")}
          </button>
          <button type="button" className="btn primary" onClick={() => void execute()}>
            {t("toolbox.alignment.confirm.action")}
          </button>
        </div>
      </Sheet>

      <Sheet
        open={forceRestartOpen}
        onDismiss={() => setForceRestartOpen(false)}
        labelledBy="alignment-force-restart-title"
        describedBy="alignment-force-restart-body"
        initialFocus="dismiss"
        centeredInExpanded
      >
        <Ring icon="refresh" variant="accent" />
        <h3 id="alignment-force-restart-title">
          {t("toolbox.alignment.forceConfirm.title")}
        </h3>
        <p id="alignment-force-restart-body">
          {t("toolbox.alignment.forceConfirm.body")}
        </p>
        <div className="row2 sheet-actions">
          <button
            type="button"
            className="btn ghost"
            onClick={() => setForceRestartOpen(false)}
          >
            {t("config.confirm.cancel")}
          </button>
          <button type="button" className="btn primary" onClick={() => void forceRestart()}>
            {t("toolbox.alignment.forceConfirm.action")}
          </button>
        </div>
      </Sheet>
    </>
  );
}
