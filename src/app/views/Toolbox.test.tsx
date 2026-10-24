import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { I18nProvider } from "../i18n";
import { Toolbox } from "./Toolbox";
import { ConversationAlignment } from "./toolbox/ConversationAlignment";
import { UiLanguage } from "./toolbox/UiLanguage";
import { TOOLS } from "./toolbox/registry";

describe("Toolbox shell", () => {
  beforeEach(() => {
    localStorage.setItem("cam.lang", "en");
  });

  it("renders tools from the typed registry", () => {
    render(
      <I18nProvider>
        <Toolbox onBack={vi.fn()} />
      </I18nProvider>,
    );

    expect(TOOLS.map((tool) => tool.id)).toEqual([
      "conversation-alignment",
      "ui-language",
    ]);
    expect(TOOLS[0].component).toBe(ConversationAlignment);
    expect(TOOLS[1].component).toBe(UiLanguage);
    expect(screen.getByRole("heading", { name: "Toolbox" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Conversation alignment/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Codex UI language/ })).toBeInTheDocument();
    expect(screen.getAllByText("Available")).toHaveLength(TOOLS.length);
  });

  it("opens the Codex UI language tool from the list", async () => {
    const user = userEvent.setup();
    render(
      <I18nProvider>
        <Toolbox onBack={vi.fn()} />
      </I18nProvider>,
    );

    await user.click(screen.getByRole("button", { name: /Codex UI language/ }));

    expect(
      screen.getByRole("heading", { name: "Codex UI language" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Follow system" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "简体中文" })).toBeInTheDocument();
  });

  it("returns to the compact parent view", async () => {
    const user = userEvent.setup();
    const onBack = vi.fn();
    render(
      <I18nProvider>
        <Toolbox onBack={onBack} />
      </I18nProvider>,
    );

    await user.click(screen.getByRole("button", { name: "Back" }));

    expect(onBack).toHaveBeenCalledOnce();
  });
});
