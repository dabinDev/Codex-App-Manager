# Configurable Branding PR #8 Integration Design

## Goal

Merge PR #8's centralized branding work into `main` without weakening session isolation,
release correctness, packaged smoke coverage, or the existing conversation-alignment tools.
The checked-in `brand.config.json` remains the source of truth for visible branding and all
external service locations.

This integration supports changing the API Base URL, application icons, product display name,
and manager-update feed without editing application code. It does not implement a new update
server or dynamic update discovery.

## Configuration Contract

`brand.config.json` owns:

- `productName`, localized `displayNames`, and `serviceName`;
- support, API, mirror, skin, and fixed manager-update manifest URLs;
- frontend light/dark logos, Tauri bundle icons, and the Windows installer icon;
- a fixed release artifact base URL used when generating `latest.json`.

The manager updater checks the configured manifest URLs in their checked-in order. The current
release implementation uses only these fixed URLs. A future self-hosted update service can replace
the configured URLs without changing the client-side update contract.

Compatibility-sensitive identifiers remain stable and are not configurable: Cargo/npm package
names, `mainBinaryName`, the Tauri bundle identifier, persisted settings names, lock names, and
other machine-facing identifiers.

## API Session Isolation

A remembered refresh token is valid only for the normalized origin of the configured API service.
Session metadata stores that origin when login succeeds. Restore is allowed only when the stored
origin exactly matches the current configured origin.

Legacy metadata without an origin, or metadata for a different origin, is treated as signed out:
the token is not sent over the network, remembered-login state is cleared, and the saved email may
remain for user convenience. A subsequent explicit login binds the new token to the new origin.

This prevents a rebranded build from silently forwarding credentials from one API operator to
another.

## Manager Update Behavior

Tauri updater endpoints are generated from the fixed manifest URLs in `brand.config.json`.
`latest.json` generation reads the configured release artifact base URL and `productName`; it must
not hardcode the upstream repository or product name.

Update checks and installs return stable, user-facing categories instead of swallowing every error
as a generic unavailable state:

- update service unreachable or timed out;
- update manifest invalid or incompatible;
- updater configuration invalid;
- download failed;
- signature or package verification failed;
- installation failed;
- unknown update failure.

The About page localizes these categories and suggests trying again later. The underlying detail
is retained for diagnostics but is not used as the primary UI copy. No new updater backend,
channel selection, rollout logic, or automatic retry policy is added in this integration.

## Icons And Packaging

Frontend icon paths may point anywhere below `src/app/assets/`, including nested directories.
Both the validator and Vite glob use the same recursive scope, so a path cannot pass configuration
validation and then fail at application startup. Paths outside that directory are rejected.

Windows and macOS packaged smoke scripts obtain the expected product name from the same brand
configuration, while continuing to use the stable binary name. macOS localized Quit menu checks
compose their labels from the localized verb and configured product name. This keeps custom product
names covered by the existing lifecycle tests.

## Merge Compatibility

The three textual conflicts are resolved by preserving both sides:

- keep the existing favicon while setting the HTML title at runtime from brand configuration;
- register both conversation-alignment modules and the new Rust brand module;
- use configured light/dark brand logos while retaining all current icon exports.

No existing alignment, restart, API-key import, local-write, backup, or recovery behavior is
removed.

## Verification

Implementation follows red-green TDD for each review finding:

1. Rust session tests prove a token is never restored across service origins, including legacy
   metadata without an origin.
2. Manifest-generation tests prove product notes and artifact URLs follow non-default branding.
3. Brand synchronization tests prove nested frontend assets are accepted and paths outside the
   asset root are rejected.
4. Packaged-smoke contract tests prove product-name expectations come from brand configuration.
5. Frontend updater tests prove each stable error category maps to distinct localized copy.

The final merge must pass brand synchronization, TypeScript checking, scoped ESLint, all tests,
the production frontend build, Rust checks available on the host, and `git diff --check`. The
repository-local `.worktrees/` directory is excluded from lint and Vitest invocation because it
contains independent complete checkouts and is not part of the current worktree.
