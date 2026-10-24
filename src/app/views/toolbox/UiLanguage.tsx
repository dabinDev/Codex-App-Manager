import { useCallback, useEffect, useRef, useState } from "react";

import {
  errorCode,
  managerApi,
  type CodexLocaleApplyResult,
} from "../../../services/managerApi";
import type { AppSettings } from "../../../shared/types";
import { Ring, Segmented, StatusBanner } from "../../components";
import { Icon } from "../../icons";
import { useI18n } from "../../i18n";
import { Sheet } from "../../Sheet";

/** Segment key ↔ persisted `codexLocale`. `system` maps to `null` (follow the
 *  OS language); every other key drives both Chromium `--lang` and Codex's
 *  official `localeOverride`. Its `autonym` is the language's own name. Add
 *  entries here and to `CODEX_LOCALE_OPTIONS` in managerApi.ts together. */
const SEGMENTS = [
  { key: "system", locale: null as string | null, autonym: null },
  { key: "zh-CN", locale: "zh-CN" as string | null, autonym: "简体中文" },
] as const;

type LoadState =
  | { kind: "loading" }
  | { kind: "ready"; settings: AppSettings }
  | { kind: "error"; code: string };

type ApplyStatus =
  | { kind: "idle" }
  | { kind: "restarting" }
  | { kind: "done"; result: CodexLocaleApplyResult }
  | { kind: "error"; code: string };

/** Which segment a persisted locale corresponds to. Unknown values fall back to
 *  "follow system" so the control never renders an unselected state. */
function segmentFor(locale: string | null): string {
  return SEGMENTS.find((s) => s.locale === locale)?.key ?? "system";
}

export function UiLanguage() {
  const { t } = useI18n();
  const [state, setState] = useState<LoadState>({ kind: "loading" });
  const [apply, setApply] = useState<ApplyStatus>({ kind: "idle" });
  const [confirmOpen, setConfirmOpen] = useState(false);
  // Visible to render so the restart action is disabled while a locale save is
  // still in flight — otherwise a restart could read the previous persisted
  // locale and report a false success.
  const [saving, setSaving] = useState(false);
  const applyInFlight = useRef(false);

  const load = useCallback(async () => {
    setState({ kind: "loading" });
    try {
      const settings = await managerApi.getSettings();
      setState({ kind: "ready", settings });
    } catch (cause) {
      setState({ kind: "error", code: errorCode(cause) ?? "unknown" });
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const choose = async (key: string) => {
    if (state.kind !== "ready" || saving) return;
    const next = SEGMENTS.find((s) => s.key === key);
    if (!next || segmentFor(state.settings.codexLocale) === key) return;
    setSaving(true);
    try {
      const saved = await managerApi.setSettings({
        ...state.settings,
        codexLocale: next.locale,
      });
      setState({ kind: "ready", settings: saved });
      setApply({ kind: "idle" });
      setConfirmOpen(true);
    } catch (cause) {
      setState({ kind: "error", code: errorCode(cause) ?? "unknown" });
    } finally {
      setSaving(false);
    }
  };

  const applyCodexLocale = async () => {
    if (applyInFlight.current) return;
    applyInFlight.current = true;
    setConfirmOpen(false);
    setApply({ kind: "restarting" });
    try {
      const result = await managerApi.toolboxApplyCodexLocale(true);
      setApply({ kind: "done", result });
    } catch (cause) {
      setApply({ kind: "error", code: errorCode(cause) ?? "unknown" });
    } finally {
      applyInFlight.current = false;
    }
  };

  const selected =
    state.kind === "ready" ? segmentFor(state.settings.codexLocale) : "system";
  const applying = apply.kind === "restarting";

  return (
    <>
      <section
        className="alignment-tool"
        aria-labelledby="codex-locale-title"
        inert={confirmOpen ? true : undefined}
      >
        <header className="alignment-heading">
          <span className="alignment-mark" aria-hidden="true">
            <Icon name="globe" />
          </span>
          <div>
            <h1 id="codex-locale-title">{t("toolbox.locale.title")}</h1>
            <p>{t("toolbox.locale.intro")}</p>
          </div>
        </header>

        <div className="alignment-privacy">
          <Icon name="info" />
          <span>{t("toolbox.locale.note")}</span>
        </div>

        {state.kind === "error" ? (
          <StatusBanner
            tone="err"
            action={
              <button type="button" className="linkbtn" onClick={() => void load()}>
                {t("toolbox.locale.retry")}
              </button>
            }
          >
            {t("toolbox.locale.loadFailed")}
          </StatusBanner>
        ) : null}

        <Segmented
          ariaLabel={t("toolbox.locale.title")}
          value={selected}
          items={SEGMENTS.map((s) => ({
            key: s.key,
            // A target language is shown by its own autonym (untranslated);
            // "system" uses the manager-localized label.
            label: s.autonym ?? t("toolbox.locale.system"),
          }))}
          onChange={(key) => void choose(key)}
        />

        <div className="alignment-force-restart">
          <button
            type="button"
            className="btn primary"
            disabled={state.kind !== "ready" || applying || saving}
            onClick={() => setConfirmOpen(true)}
          >
            <Icon name={applying ? "loader" : "refresh"} />
            {t(applying ? "toolbox.locale.restarting" : "toolbox.locale.restart")}
          </button>

          {apply.kind === "done" ? (
            <StatusBanner tone="ok">
              {apply.result.mode === "official_catalog"
                ? t("toolbox.locale.officialDone")
                : apply.result.mode === "compatibility_runtime"
                  ? t("toolbox.locale.compatibilityDone", {
                      count: apply.result.translatedNodes,
                    })
                  : t("toolbox.locale.systemDone")}
            </StatusBanner>
          ) : null}
          {apply.kind === "error" ? (
            <StatusBanner tone="err">
              {apply.code === "alignment_busy"
                ? t("toolbox.locale.busy")
                : apply.code === "codex_locale_zero_translated_ui"
                  ? t("toolbox.locale.zeroTranslated")
                  : apply.code === "codex_locale_setting_verification_failed"
                    ? t("toolbox.locale.settingFailed")
                    : apply.code === "codex_locale_cdp_unavailable"
                      ? t("toolbox.locale.cdpFailed")
                      : apply.code === "codex_locale_runtime_install_failed"
                        ? t("toolbox.locale.runtimeFailed")
                        : t("toolbox.locale.restartFailed")}
            </StatusBanner>
          ) : null}
        </div>
      </section>

      <Sheet
        open={confirmOpen}
        onDismiss={() => setConfirmOpen(false)}
        labelledBy="codex-locale-confirm-title"
        describedBy="codex-locale-confirm-body"
        initialFocus="dismiss"
        centeredInExpanded
      >
        <Ring icon="refresh" variant="accent" />
        <h3 id="codex-locale-confirm-title">{t("toolbox.locale.confirm.title")}</h3>
        <p id="codex-locale-confirm-body">{t("toolbox.locale.confirm.body")}</p>
        <div className="row2 sheet-actions">
          <button type="button" className="btn ghost" onClick={() => setConfirmOpen(false)}>
            {t("config.confirm.cancel")}
          </button>
          <button type="button" className="btn primary" onClick={() => void applyCodexLocale()}>
            {t("toolbox.locale.confirm.action")}
          </button>
        </div>
      </Sheet>
    </>
  );
}
