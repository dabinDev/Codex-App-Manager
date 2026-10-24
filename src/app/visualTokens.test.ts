import { describe, expect, it } from "vitest";

import styles from "./styles.css?inline";

function tokenHex(name: string) {
  const match = styles.match(new RegExp(`${name}:\\s*(#[0-9a-f]{6});`, "i"));
  if (!match) throw new Error(`Missing hex token: ${name}`);
  return match[1];
}

function relativeLuminance(hex: string) {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)!
    .map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) =>
      channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
    );
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrastRatio(first: string, second: string) {
  const [lighter, darker] = [relativeLuminance(first), relativeLuminance(second)].sort(
    (a, b) => b - a,
  );
  return (lighter + 0.05) / (darker + 0.05);
}

describe("application visual tokens", () => {
  it("uses the approved Acid Orange home palette", () => {
    expect(styles).toContain("--brand-orange: #ff6b1a;");
    expect(styles).toContain("--brand-acid: #b8d83d;");
    expect(styles).toContain("--brand-teal: #0f8f83;");
    expect(styles).toContain("--brand-graphite: #18201e;");
    expect(styles).toContain("--brand-mist: #f4f7f5;");
    expect(styles).toContain("--brand-muted: #66716d;");
    expect(styles).toContain("--brand-line: #d9e0dc;");
    expect(styles).toContain("--brand-paper: #ffffff;");
    expect(styles).toContain("--accent: var(--brand-orange);");
    expect(styles).not.toContain("--brand-blue:");
  });

  it("keeps semantic colors independent from the interaction accent", () => {
    expect(styles).toContain("--success: #22c55e;");
    expect(styles).toContain("--amber: #f59e0b;");
    expect(styles).toContain("--danger: #ef4444;");
  });

  it("keeps pressed primary controls readable at normal text size", () => {
    expect(contrastRatio(tokenHex("--accent-pressed"), tokenHex("--brand-graphite"))).toBeGreaterThanOrEqual(
      4.5,
    );
  });

  it("caps card and control radii and removes decorative background effects", () => {
    expect(styles).toContain("--r-lg: 8px;");
    expect(styles).toContain("--r-md: 8px;");
    expect(styles).toContain("--r-sm: 6px;");
    expect(styles).not.toContain("--grain:");
    expect(styles).not.toContain("--ambient:");
  });

  it("defines every visual token referenced by the stylesheet", () => {
    const referenced = [...styles.matchAll(/var\(--([a-z0-9-]+)/g)].map((match) => match[1]);
    const defined = new Set(
      [...styles.matchAll(/--([a-z0-9-]+)\s*:/g)].map((match) => match[1]),
    );
    const missing = [...new Set(referenced)].filter((token) => !defined.has(token));

    expect(missing).toEqual([]);
  });
});
