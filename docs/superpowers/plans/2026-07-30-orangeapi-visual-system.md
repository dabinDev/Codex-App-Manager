# OrangeAPI-Inspired Visual System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refresh the complete application with a restrained blue/neutral color system and one consistent outline icon family while preserving every existing workflow and layout.

**Architecture:** Keep the existing semantic CSS-token boundary and stable `Icon` API. Replace token values and hard-coded legacy accents centrally, then map stable icon names to `lucide-react` components so feature views remain decoupled from the icon package.

**Tech Stack:** React 19, TypeScript 6, CSS custom properties, `lucide-react`, Vitest/Testing Library, Playwright/browser visual checks.

---

## File Map

- Modify `package.json` and `package-lock.json`: add `lucide-react`.
- Modify `src/app/icons.tsx`: replace hand-authored interface paths with a typed Lucide map while retaining brand artwork.
- Modify `src/app/icons.test.tsx`: cover icon mapping, accessibility, and stable classes.
- Modify `src/app/styles.css`: replace global color/material tokens and hard-coded purple accents.
- Modify affected view tests: preserve semantic states and navigation behavior.

### Task 1: Lock the global token contract

**Files:**
- Create: `src/app/visualTokens.test.ts`
- Modify: `src/app/styles.css`

- [ ] **Step 1: Write a failing static token test**

Read `styles.css` as text and require the approved anchors:

```ts
expect(styles).toContain("--accent: #3b82f6;");
expect(styles).toContain("--accent-hover: #2563eb;");
expect(styles).toContain("--accent-pressed: #1d4ed8;");
expect(styles).not.toMatch(/--brand-violet|oklch\([^)]*28[0-9]/i);
```

Also require semantic success/warning/danger tokens and maximum 8px card/control
radii, excluding the native window shell and pill/circle controls.

- [ ] **Step 2: Run the token test and verify RED**

```powershell
npx vitest run src/app/visualTokens.test.ts
```

Expected: assertions fail against the current violet OKLCH system.

- [ ] **Step 3: Replace color and material tokens**

Use solid neutral surfaces and blue interaction states:

```css
:root {
  --brand-blue: #3b82f6;
  --accent: #3b82f6;
  --accent-hover: #2563eb;
  --accent-pressed: #1d4ed8;
  --r-lg: 8px;
  --r-md: 8px;
  --r-sm: 6px;
}
```

Define separate light and dark neutral surfaces, borders, text, semantic colors,
focus rings, and restrained shadows. Remove decorative ambient/noise layers and
purple hard-coded selectors. Keep user-selected Codex skin preview colors
unchanged because they represent external theme content.

- [ ] **Step 4: Run token and component tests**

```powershell
npx vitest run src/app/visualTokens.test.ts src/app/App.test.tsx src/app/Rail.test.tsx
```

Expected: token contract and core shell tests pass.

- [ ] **Step 5: Commit the color system**

```powershell
git add -- src/app/styles.css src/app/visualTokens.test.ts
git commit -m "style: adopt blue neutral application palette"
```

### Task 2: Migrate interface icons behind the stable adapter

**Files:**
- Modify: `package.json`
- Modify: `package-lock.json`
- Modify: `src/app/icons.tsx`
- Create: `src/app/icons.test.tsx`

- [ ] **Step 1: Write failing icon-adapter tests**

Require stable classes, hidden decorative SVG semantics, consistent geometry,
and the new Toolbox icon names:

```tsx
render(<Icon name="wrench" />);
const svg = document.querySelector("svg");
expect(svg).toHaveClass("cam-icon", "cam-icon-wrench");
expect(svg).toHaveAttribute("aria-hidden", "true");
expect(svg).toHaveAttribute("stroke-width", "1.75");
```

- [ ] **Step 2: Run the icon test and verify RED**

```powershell
npx vitest run src/app/icons.test.tsx
```

Expected: missing icon names and old stroke geometry fail.

- [ ] **Step 3: Add Lucide and implement the typed map**

Install the dependency with npm so both manifests update. Map every existing
`IconName` plus `wrench`, `messages`, `scan`, `archive`, and `undo` to a Lucide
component:

```tsx
const ICONS = {
  house: House,
  key: KeyRound,
  palette: Palette,
  wrench: Wrench,
  messages: MessagesSquare,
} as const satisfies Record<IconName, LucideIcon>;
```

Render through the existing `Icon` function with `size={24}`,
`strokeWidth={1.75}`, and stable `cam-icon-*` classes. Preserve `CodexMark` and
`CodexGlyph` exactly.

- [ ] **Step 4: Run icon and frontend tests**

```powershell
npx vitest run src/app/icons.test.tsx src/app/App.test.tsx src/app/Rail.test.tsx
npm run check
```

Expected: all tests and TypeScript pass without changing view-facing icon calls.

- [ ] **Step 5: Commit the icon migration**

```powershell
git add -- package.json package-lock.json src/app/icons.tsx src/app/icons.test.tsx
git commit -m "style: unify application icons with Lucide"
```

### Task 3: Perform visual and accessibility verification

**Files:**
- Modify only selectors proven defective by verification.

- [ ] **Step 1: Run static and behavioral gates**

```powershell
npm run check
npm run lint
npx vitest run --maxWorkers=1 --testTimeout=15000
npm run build
```

Expected: zero failures.

- [ ] **Step 2: Inspect representative views in both themes**

Run the Vite app and inspect Home, API Configuration login/list, Themes,
Toolbox list/detail, and Settings at `1100x720`, `960x640`, and `400x640`.
Capture screenshots in light and dark modes. Check text contrast, focus rings,
selection states, semantic banners, icon alignment, scrolling, and fixed control
sizes.

- [ ] **Step 3: Run automated overlap and overflow checks**

For each viewport require:

```js
document.documentElement.scrollWidth <= document.documentElement.clientWidth
document.documentElement.scrollHeight <= document.documentElement.clientHeight
```

Check every visible button's text and SVG bounding boxes remain inside its own
bounding box. Require no console errors and nonblank rendered screenshots.

- [ ] **Step 4: Fix only evidenced visual defects and repeat checks**

For each defect, add a focused regression assertion where DOM geometry or class
state can express it, change the narrowest selector, and rerun the affected test
plus screenshot. Do not alter OrangeAPI source or user skin package colors.

- [ ] **Step 5: Commit verified visual corrections**

```powershell
git add -- src/app/styles.css src/app/**/*.test.tsx src/app/**/*.test.ts
git commit -m "fix: polish responsive visual states"
```
