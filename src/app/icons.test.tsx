import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { BRAND_LOGO_DARK, BRAND_LOGO_LIGHT } from "./brand";
import { CodexMark, Icon, type IconName } from "./icons";
import { ThemeProvider } from "./theme";

describe("Icon adapter", () => {
  it("keeps stable classes and decorative SVG semantics", () => {
    const { container } = render(<Icon name={"wrench" as IconName} />);
    const svg = container.querySelector("svg");

    expect(svg).toHaveClass("cam-icon", "cam-icon-wrench");
    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(svg).toHaveAttribute("stroke-width", "1.75");
  });

  it.each(["wrench", "messages", "scan", "archive", "undo"] as const)(
    "renders the %s Toolbox icon",
    (name) => {
      const { container } = render(<Icon name={name as IconName} />);
      const svg = container.querySelector("svg");

      expect(svg?.childElementCount).toBeGreaterThan(0);
    },
  );

  it.each([
    ["dark", BRAND_LOGO_DARK],
    ["light", BRAND_LOGO_LIGHT],
  ] as const)("renders the configured %s brand mark", (theme, expectedLogo) => {
    localStorage.setItem("cam.theme", theme);
    const { container } = render(
      <ThemeProvider>
        <CodexMark />
      </ThemeProvider>,
    );

    expect(container.querySelector("img")).toHaveAttribute("src", expectedLogo);
  });
});
