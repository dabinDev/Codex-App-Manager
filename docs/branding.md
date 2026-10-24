# Brand configuration

`brand.config.json` is the source of truth for product-facing branding:

- `productName`: the OS-level product and window name.
- `serviceName`: the default API service name.
- `displayNames`: the in-app name for every supported locale.
- `sites`: support, API service, Codex mirror, skin catalog, and manager updater URLs.
- `updates.artifactBaseUrl`: fixed release artifact root used to generate updater manifests.
- `icons.frontend`: light and dark in-app logos.
- `icons.bundle`: Tauri application icons.
- `icons.windowsInstaller`: the NSIS installer icon.

Site URLs must use HTTPS, must not contain credentials, query strings, or
fragments, and must not end with `/`. Runtime code appends its own paths to
the configured service roots.

Manager self-update checks only the fixed `sites.updater` manifest URLs, in
order. `updates.artifactBaseUrl` controls the artifact links written into
`latest.json`; it does not enable dynamic update discovery or a separate
update service.

After editing the file, run:

```bash
npm run brand:sync
npm run check
```

The sync command validates all URLs and icon paths, then updates the
brand-derived fields in `src-tauri/tauri.conf.json`. The canonical
`npm run tauri:dev`, `npm run tauri:build`, and `npm run tauri build`
commands run this sync automatically.

The Rust build reads the same file through `src-tauri/build.rs`, while the
frontend imports it directly. `npm run brand:check` fails when the checked-in
Tauri configuration is stale.

The API service name can be overridden at build time without editing the
checked-in configuration:

```bash
BRAND_SERVICE_NAME="My API" npm run tauri:dev
```

The canonical frontend and Tauri build commands also read
`BRAND_SERVICE_NAME` from a root `.env` file. Empty overrides are ignored and
fall back to `brand.config.json`.

Compatibility-sensitive internal identifiers are intentionally not part of
the brand configuration. Do not change `mainBinaryName`, the bundle
`identifier`, Cargo/package names, persisted storage keys, or lock-file names
for a visible rebrand.
