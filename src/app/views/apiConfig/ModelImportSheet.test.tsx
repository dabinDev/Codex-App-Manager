import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../../services/managerApi";
import type { ApiConfigKey } from "../../../shared/types";
import { I18nProvider } from "../../i18n";
import { ThemeProvider } from "../../theme";
import { ModelImportSheet } from "./ModelImportSheet";

vi.mock("../../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      ...actual.managerApi,
      apiConfigModels: vi.fn(),
      apiConfigTraeCodeModels: vi.fn(),
      apiConfigProbeTraeCode: vi.fn(),
      apiConfigImportTraeCode: vi.fn(),
    },
  };
});

type TraeCodeManagerApi = typeof managerApi & {
  apiConfigTraeCodeModels: (keyId: number) => Promise<string[]>;
};

const api = managerApi as TraeCodeManagerApi;

const KEY: ApiConfigKey = {
  id: 7,
  name: "Grok key",
  groupName: "Grok",
  maskedKey: "x••••123",
  status: "active",
  quota: 100,
  quotaUsed: 0,
  expiresAt: null,
  actionable: true,
  enabled: false,
};

describe("ModelImportSheet", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
    vi.mocked(managerApi.apiConfigModels).mockReset().mockResolvedValue(["gpt-5.5"]);
    api.apiConfigTraeCodeModels = vi.fn().mockResolvedValue([
      "grok-4",
      "gemini-2.5-pro",
      "claude-sonnet-4",
    ]);
  });

  it("loads TraeCode models from the selected key instead of the Codex GPT catalog", async () => {
    render(
      <ThemeProvider>
        <I18nProvider>
          <ModelImportSheet
            destination="traecode"
            apiKey={KEY}
            onClose={vi.fn()}
            onCodexReport={vi.fn()}
          />
        </I18nProvider>
      </ThemeProvider>,
    );

    expect(await screen.findByText("grok-4")).toBeInTheDocument();
    expect(screen.getByText("gemini-2.5-pro")).toBeInTheDocument();
    expect(screen.getByText("claude-sonnet-4")).toBeInTheDocument();
    expect(screen.queryByText("gpt-5.5")).not.toBeInTheDocument();
    await waitFor(() => expect(api.apiConfigTraeCodeModels).toHaveBeenCalledWith(KEY.id));
    expect(managerApi.apiConfigModels).not.toHaveBeenCalled();
  });

  it("shows only GPT models in the Codex model selector", async () => {
    vi.mocked(managerApi.apiConfigModels).mockResolvedValue(["gpt-6.1-sol", "grok-3", "gemini-2.5-pro", "claude-sonnet-4"]);
    render(
      <ThemeProvider><I18nProvider>
        <ModelImportSheet destination="codex" apiKey={KEY} onClose={vi.fn()} onCodexReport={vi.fn()} />
      </I18nProvider></ThemeProvider>,
    );
    expect(await screen.findByText("gpt-6.1-sol")).toBeVisible();
    for (const model of ["grok-3", "gemini-2.5-pro", "claude-sonnet-4"]) {
      expect(screen.queryByText(model)).not.toBeInTheDocument();
    }
    expect(api.apiConfigTraeCodeModels).not.toHaveBeenCalled();
  });
});
