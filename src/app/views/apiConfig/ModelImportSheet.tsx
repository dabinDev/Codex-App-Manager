import { useEffect, useId, useMemo, useState } from "react";

import { errorCode, managerApi } from "../../../services/managerApi";
import type {
  ApiConfigDestination,
  ApiConfigKey,
  CodexModelImportReport,
  TraeCodeImportReport,
  TraeCodeProbe,
} from "../../../shared/types";
import { Icon } from "../../icons";
import { useI18n } from "../../i18n";
import { Sheet } from "../../Sheet";
import { apiConfigErrorText } from "./errors";

export function ModelImportSheet({
  destination,
  apiKey,
  onClose,
  onCodexReport,
}: {
  destination: ApiConfigDestination;
  apiKey: ApiConfigKey;
  onClose: () => void;
  onCodexReport: (keyId: number, report: CodexModelImportReport) => void;
}) {
  const { t } = useI18n();
  const titleId = useId();
  const descriptionId = useId();
  const [models, setModels] = useState<string[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [defaultModel, setDefaultModel] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [codexCloseConfirmed, setCodexCloseConfirmed] = useState(false);
  const [traeProbe, setTraeProbe] = useState<TraeCodeProbe | null>(null);
  const [traeReport, setTraeReport] = useState<TraeCodeImportReport | null>(null);

  useEffect(() => {
    let cancelled = false;
    setModels(null);
    setSelected(new Set());
    setDefaultModel(null);
    setLoadError(null);
    setSubmitError(null);
    setCodexCloseConfirmed(false);
    setTraeProbe(null);
    setTraeReport(null);
    const loadModels = destination === "codex"
      ? managerApi.apiConfigModels(apiKey.id)
      : managerApi.apiConfigTraeCodeModels(apiKey.id);
    void loadModels
      .then((items) => {
        if (!cancelled) {
          const available = destination === "codex"
            ? items.filter((model) => model.trim().toLowerCase().startsWith("gpt-"))
            : items;
          setModels(available);
          if (destination === "codex") {
            setSelected(new Set(available));
            setDefaultModel(available[0] ?? null);
          }
        }
      })
      .catch((cause) => {
        if (!cancelled) setLoadError(errorCode(cause) ?? "unknown");
      });
    return () => {
      cancelled = true;
    };
  }, [apiKey.id, destination, reloadToken]);

  const selectedModels = useMemo(
    () => models?.filter((model) => selected.has(model)) ?? [],
    [models, selected],
  );

  const toggleModel = (model: string, checked: boolean) => {
    setTraeProbe(null);
    setTraeReport(null);
    setSelected((current) => {
      const next = new Set(current);
      if (checked) next.add(model);
      else next.delete(model);
      setDefaultModel((currentDefault) => {
        if (checked && currentDefault == null) return model;
        if (!checked && currentDefault === model) {
          return models?.find((candidate) => next.has(candidate)) ?? null;
        }
        return currentDefault;
      });
      return next;
    });
  };

  const submitCodex = async () => {
    if (
      destination !== "codex" ||
      submitting ||
      selectedModels.length === 0 ||
      !codexCloseConfirmed
    ) {
      return;
    }
    const importDefaultModel = defaultModel ?? selectedModels[0];
    if (!importDefaultModel) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      const report = await managerApi.apiConfigImportCodex(
        apiKey.id,
        selectedModels,
        importDefaultModel,
      );
      onCodexReport(apiKey.id, report);
    } catch (cause) {
      setSubmitError(errorCode(cause) ?? "unknown");
    } finally {
      setSubmitting(false);
    }
  };

  const reviewTraeCode = async () => {
    if (destination !== "traecode" || submitting || selectedModels.length === 0) {
      return;
    }
    setSubmitting(true);
    setSubmitError(null);
    setTraeReport(null);
    try {
      const probe = await managerApi.apiConfigProbeTraeCode();
      if (!probe.installed) {
        setSubmitError("traecode_not_installed");
        return;
      }
      setTraeProbe(probe);
    } catch (cause) {
      setSubmitError(errorCode(cause) ?? "unknown");
    } finally {
      setSubmitting(false);
    }
  };

  const submitTraeCode = async () => {
    if (
      destination !== "traecode" ||
      submitting ||
      selectedModels.length === 0 ||
      traeProbe == null
    ) {
      return;
    }
    setSubmitting(true);
    setSubmitError(null);
    try {
      const report = await managerApi.apiConfigImportTraeCode(
        apiKey.id,
        selectedModels,
        traeProbe.restartRequired,
      );
      setTraeReport(report);
      setTraeProbe(null);
    } catch (cause) {
      const code = errorCode(cause) ?? "unknown";
      setSubmitError(code);
      if (code === "traecode_restart_required") setTraeProbe(null);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Sheet
      open
      onDismiss={onClose}
      dismissable={!submitting}
      labelledBy={titleId}
      describedBy={descriptionId}
      initialFocus="container"
      centeredInExpanded
    >
      <div className="api-model-sheet">
      <div className="api-model-sheet-heading">
        <div>
          <h3 id={titleId}>
            {t(destination === "codex" ? "config.modelsTitleCodex" : "config.modelsTitleTraeCode")}
          </h3>
          <p id={descriptionId}>
            {apiKey.name} · {apiKey.groupName}
          </p>
        </div>
      </div>

      {models == null && loadError == null ? (
        <div className="api-model-state" role="status">
          <Icon name="loader" />
          <span>{t("config.modelsLoading")}</span>
        </div>
      ) : null}

      {loadError ? (
        <div className="api-model-load-error">
          <div className="banner err" role="alert">
            <Icon name="alert" />
            <span>{apiConfigErrorText(loadError, t)}</span>
          </div>
          <button
            className="btn ghost"
            type="button"
            onClick={() => {
              setModels(null);
              setLoadError(null);
              setReloadToken((value) => value + 1);
            }}
          >
            {t("config.retry")}
          </button>
        </div>
      ) : null}

      {models?.length === 0 ? (
        <div className="api-model-state empty">
          <Icon name="search" />
          <span>{t("config.modelsEmpty")}</span>
        </div>
      ) : null}

      {models && models.length > 0 ? (
        <div className="api-model-list">
          {models.map((model) => {
            const checked = selected.has(model);
            return (
              <div className="api-model-row" key={model}>
                <label>
                  {destination === "traecode" ? (
                    <input
                      type="checkbox"
                      checked={checked}
                      disabled={submitting}
                      onChange={(event) => toggleModel(model, event.currentTarget.checked)}
                    />
                  ) : null}
                  <code>{model}</code>
                </label>
              </div>
            );
          })}
        </div>
      ) : null}

      {destination === "codex" && models && models.length > 0 ? (
        <label className="api-model-codex-warning">
          <input
            type="checkbox"
            checked={codexCloseConfirmed}
            disabled={submitting}
            onChange={(event) => setCodexCloseConfirmed(event.currentTarget.checked)}
          />
          <span>{t("config.confirm.closeWarning")}</span>
        </label>
      ) : null}

      {destination === "traecode" ? (
        <div className="api-model-replacement-warning">
          <Icon name="alert" />
          <span>
            {t("config.traeReplacementWarning")}
          </span>
        </div>
      ) : null}

      {traeProbe ? (
        <div className="api-model-confirm" role="alert">
          <strong>{t("config.traeConfirmTitle")}</strong>
          <span>
            {traeProbe.restartRequired
              ? t("config.traeRestartConfirmation")
              : t("config.traeReadyConfirmation")}
          </span>
        </div>
      ) : null}

      {traeReport ? (
        <div
          className={`api-model-result ${traeReport.outcome}`}
          role={traeReport.outcome === "committed" ? "status" : "alert"}
        >
          <strong>
            {traeReport.outcome === "committed"
              ? t("config.traeCommitted", { n: traeReport.addedModels.length })
              : traeReport.outcome === "partial"
                ? t("config.traePartial", {
                    added: traeReport.addedModels.length,
                    failed: traeReport.failedModels.length,
                  })
                : t("config.traeNoChanges")}
          </strong>
          {traeReport.failedModels.length > 0 ? (
            <ul>
              {traeReport.failedModels.map((failure) => (
                <li key={`${failure.modelId}:${failure.errorCode}`}>
                  <code>{failure.modelId}</code>
                  <span>{apiConfigErrorText(failure.errorCode, t)}</span>
                </li>
              ))}
            </ul>
          ) : null}
          {traeReport.errorCode &&
          !traeReport.failedModels.some(
            (failure) => failure.errorCode === traeReport.errorCode,
          ) ? (
            <span className="api-model-result-error">
              {apiConfigErrorText(traeReport.errorCode, t)}
            </span>
          ) : null}
        </div>
      ) : null}

      {submitError ? (
        <div className="banner err" role="alert">
          <Icon name="alert" />
          <span>{apiConfigErrorText(submitError, t)}</span>
        </div>
      ) : null}

      <div className="api-model-selection-count" role="status">
        {t("config.modelsSelected", { n: selectedModels.length })}
      </div>
      <div className="row2 sheet-actions">
        <button
          className="btn ghost"
          type="button"
          data-sheet-dismiss
          disabled={submitting}
          onClick={onClose}
        >
          {t("config.cancel")}
        </button>
        <button
          className="btn primary"
          type="button"
          data-sheet-primary
          disabled={
            submitting ||
            selectedModels.length === 0 ||
            (destination === "codex" && !codexCloseConfirmed)
          }
          onClick={() =>
            void (destination === "codex"
              ? submitCodex()
              : traeProbe
                ? submitTraeCode()
                : reviewTraeCode())
          }
        >
          <Icon name={submitting ? "loader" : "download"} />
          {submitting
            ? t("config.importingModels")
            : destination === "codex"
              ? t("config.importToCodex")
              : traeProbe?.restartRequired
                ? t("config.restartTraeCodeReplace")
                : traeProbe
                  ? t("config.replaceTraeCodeModels")
                  : t("config.reviewTraeCodeImport")}
        </button>
      </div>
      </div>
    </Sheet>
  );
}
