# Orange Downloader Brand Design

Date: 2026-07-31
Status: Approved by the user's explicit implementation request

## Objective

Apply the Acid Orange identity from `E:\ForkProject\orangeapi` to Codex App
Manager while preserving the desktop tool's existing product name, navigation,
workflows, installer behavior, and update identity.

## Scope

- Keep `Codex App Manager` as the product, window, binary, and installer name.
- Keep the existing installation, update, API configuration, and Codex provider
  behavior unchanged.
- Keep the OrangeAPI login endpoint fixed at `https://token.cylonai.cn`.
- Use only `orangeapi/frontend/public/logo.png` and `logo.svg` as the Acid
  Orange source artwork. The legacy `orangeapi/assets/logo.svg` Sub2API mark is
  explicitly excluded.
- Replace the application brand image, generated Windows/macOS/Linux platform
  icons, installer icon, and NSIS header/sidebar artwork.
- Replace the global blue/neutral visual tokens with the OrangeAPI home palette:
  orange `#ff6b1a`, acid `#b8d83d`, teal `#0f8f83`, graphite `#18201e`, mist
  `#f4f7f5`, muted `#66716d`, line `#d9e0dc`, and paper `#ffffff`.
- Preserve semantic danger and warning differentiation, accessibility, compact
  and expanded layouts, and the existing 8px maximum control/card radius.

## Visual Direction

The application remains a dense desktop operations tool. Orange is the primary
command and selection color, teal is the focus and connected-state signal, acid
is a restrained highlight, and graphite/mist form the neutral foundation. Dark
mode uses graphite-derived surfaces; light mode uses paper and mist. The Acid
Orange citrus-and-pointer mark is shown without a decorative enclosing card.

The installer keeps its current NSIS layout and dimensions. Its artwork changes
to a clean graphite/mist composition using the same mark and color system,
without marketing illustrations or the old blue-purple cloud icon.

## Architecture

- `src/app/styles.css` remains the single source of application visual tokens.
- `src/app/assets/orange-logo.png` is the canonical renderer brand asset.
- `CodexMark` becomes theme-independent because the Acid Orange mark works on
  both light and dark surfaces.
- `src-tauri/icons/` is generated from the canonical 512x512 transparent PNG by
  the Tauri icon generator.
- `src-tauri/installer/header.bmp` and `sidebar.bmp` are generated at their
  existing dimensions from the same palette and mark.
- A static brand contract test protects the exact source logo hash, required
  platform outputs, installer dimensions, CSS palette, and fixed login host.

## Safety

Automated and visual tests run in browser/mock mode or against isolated temporary
directories. They must not invoke the real local-write or restart commands and
must not modify `%USERPROFILE%\.codex`, macOS `~/.codex`, credential stores, or
the installed Codex application. Packaging reads project files only.

## Verification

- Focused brand contract tests first fail against the blue theme and old logo.
- TypeScript, ESLint, all non-environmental Vitest suites, and production build.
- Rust formatting/metadata and available tests without touching user data.
- Browser screenshots at expanded and compact viewport sizes in light and dark
  themes, with console, overflow, clipping, and nonblank checks.
- Inspect the generated EXE icon and installer artwork before copying the NSIS
  installer to the desktop.
- Run `codex review --base main` and address actionable findings before push.
