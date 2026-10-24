import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { managerApi } from "../../services/managerApi";
import { CATALOG, I18nProvider } from "../i18n";
import { ThemeProvider } from "../theme";
import { About } from "./About";

vi.mock("../../services/managerApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../services/managerApi")>();
  return {
    ...actual,
    managerApi: {
      checkManagerUpdate: vi.fn(),
      openUrl: vi.fn(),
      openLogsDir: vi.fn(),
      getDiagnostics: vi.fn(),
    },
  };
});

const api = vi.mocked(managerApi);

function renderAbout() {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <About onBack={vi.fn()} />
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe("manager update errors", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "zh-CN");
    api.checkManagerUpdate.mockReset();
  });

  it.each([
    ["network", "error.network"],
    ["artifact", "error.artifact"],
    ["signature", "error.signature"],
  ] as const)("shows localized %s failures", async (code, copyKey) => {
    api.checkManagerUpdate.mockRejectedValueOnce({ code, message: `raw ${code}` });
    renderAbout();

    await userEvent.click(
      screen.getByRole("button", { name: CATALOG["zh-CN"]["about.checkManager"] }),
    );

    expect(await screen.findByText(CATALOG["zh-CN"][copyKey])).toBeInTheDocument();
    expect(screen.queryByText(`raw ${code}`)).not.toBeInTheDocument();
  });
});
