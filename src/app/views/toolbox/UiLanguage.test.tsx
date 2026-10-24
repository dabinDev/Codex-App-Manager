import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../../services/managerApi";
import { DEFAULT_SETTINGS } from "../../../shared/types";
import { I18nProvider } from "../../i18n";
import { UiLanguage } from "./UiLanguage";

function renderLanguage() {
  return render(
    <I18nProvider>
      <UiLanguage />
    </I18nProvider>,
  );
}

describe("Codex UI language", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    vi.restoreAllMocks();
    vi.spyOn(managerApi, "getSettings").mockResolvedValue({ ...DEFAULT_SETTINGS });
    vi.spyOn(managerApi, "setSettings").mockImplementation(async (settings) => settings);
    vi.spyOn(managerApi, "toolboxApplyCodexLocale").mockResolvedValue({
      mode: "official_catalog",
      locale: "zh-CN",
      translatedNodes: 0,
      catalogEntries: 0,
    });
  });

  it("applies Simplified Chinese through the verified locale command", async () => {
    const user = userEvent.setup();
    renderLanguage();

    await user.click(await screen.findByRole("radio", { name: "简体中文" }));
    await user.click(await screen.findByRole("button", { name: "Restart now" }));

    expect(managerApi.toolboxApplyCodexLocale).toHaveBeenCalledWith(true);
    expect(
      await screen.findByText("Codex is using its official Simplified Chinese catalog."),
    ).toBeInTheDocument();
  });

  it("reports the verified compatibility runtime and translated node count", async () => {
    const user = userEvent.setup();
    vi.mocked(managerApi.toolboxApplyCodexLocale).mockResolvedValue({
      mode: "compatibility_runtime",
      locale: "zh-CN",
      translatedNodes: 12,
      catalogEntries: 130,
    });
    renderLanguage();

    await user.click(await screen.findByRole("radio", { name: "简体中文" }));
    await user.click(await screen.findByRole("button", { name: "Restart now" }));

    expect(
      await screen.findByText(
        "Codex is using compatibility Chinese localization (12 UI items verified).",
      ),
    ).toBeInTheDocument();
  });

  it("does not report success when renderer verification fails", async () => {
    const user = userEvent.setup();
    vi.mocked(managerApi.toolboxApplyCodexLocale).mockRejectedValue({
      code: "codex_locale_zero_translated_ui",
    });
    renderLanguage();

    await user.click(await screen.findByRole("radio", { name: "简体中文" }));
    await user.click(await screen.findByRole("button", { name: "Restart now" }));

    expect(
      await screen.findByText(/did not translate any visible Codex UI/),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Codex is using its official Simplified Chinese catalog."),
    ).not.toBeInTheDocument();
  });

  it("persists null and applies system default through the same command", async () => {
    const user = userEvent.setup();
    vi.mocked(managerApi.toolboxApplyCodexLocale).mockResolvedValue({
      mode: "system_default",
      locale: null,
      translatedNodes: 0,
      catalogEntries: 0,
    });
    vi.mocked(managerApi.getSettings).mockResolvedValue({
      ...DEFAULT_SETTINGS,
      codexLocale: "zh-CN",
    });
    renderLanguage();

    await user.click(await screen.findByRole("radio", { name: "Follow system" }));
    await waitFor(() =>
      expect(managerApi.setSettings).toHaveBeenCalledWith(
        expect.objectContaining({ codexLocale: null }),
      ),
    );
    await user.click(await screen.findByRole("button", { name: "Restart now" }));

    expect(managerApi.toolboxApplyCodexLocale).toHaveBeenCalledWith(true);
    expect(await screen.findByText("Codex is following the system language.")).toBeInTheDocument();
  });
});
