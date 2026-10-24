# Signed Branded Windows Distribution Design

## Goal

Prevent acid-orange and rotten-tomato desktop deliverables from being presented
as directly installable Windows packages unless the final installer has a valid,
trusted Authenticode signature. Preserve unsigned development builds and CI
compile checks, but make the explicit distributable packaging path fail closed.

This design cannot create or obtain a trusted certificate. Production output is
blocked until an OV/EV PFX is supplied through the existing secret interface or
the pending SignPath Foundation application is approved and integrated.

## Confirmed Root Cause

Smart App Control is enabled on the current machine. Both current brand
installers report `NotSigned`, and the acid-orange screenshot states that the
publisher cannot be verified. The rotten-tomato package previously running does
not prove it is signed; reputation and prior execution are file-specific and are
not a distribution guarantee.

The repository already contains optional PFX signing and verification scripts.
When no certificate is configured, signing currently exits successfully and
verification defaults to non-blocking. That behavior is appropriate for normal
development but unsafe for a command whose output is copied to the desktop for
distribution.

## Chosen Architecture

Keep the existing `npm run tauri:build` behavior for development. Add a separate
Windows distributable entry point that always operates in required-signature
mode. It accepts one already-isolated brand worktree, builds from its synchronized
`brand.config.json`, invokes the configured signing provider, verifies the final
bytes, archives the result under the commit-specific artifact root, and only
then copies the installer to the requested delivery directory.

The two brands continue to build in separate worktrees based on the same `main`
commit. The command does not change brand values in `main`, does not change the
shared application identifier, and does not attempt parallel installation of
both brands.

## Signing Providers

The initial provider is the existing PFX interface:

- `WINDOWS_CERTIFICATE`: base64-encoded trusted code-signing PFX;
- `WINDOWS_CERTIFICATE_PASSWORD`: its password;
- `WINDOWS_TIMESTAMP_URL`: optional RFC 3161 timestamp endpoint;
- an optional expected publisher substring used by verification.

The distributable entry point treats a missing certificate, failed import,
missing signing tool, signing error, invalid timestamp, unexpected publisher,
or invalid post-signature status as fatal. It never falls back to unsigned
output.

SignPath remains a separate future provider. It must follow the public signing
policy, trusted-build origin verification, and manual approval requirements.
The PFX path must not claim SignPath approval.

## Build And Delivery Flow

For each brand:

1. verify that the worktree is based on the requested `main` commit and has no
   unrelated changes;
2. synchronize and validate `brand.config.json`;
3. build the production NSIS target with `tauri/custom-protocol`;
4. sign the final installer through the configured provider;
5. verify Authenticode status, publisher, timestamp, architecture, product name,
   version, and production renderer embedding;
6. copy the verified installer into
   `E:\ForkProject\BuildArtifacts\Codex-App-Manager\<commit>\<brand>\signed`;
7. compute SHA-256 for the source and archived copy and require equality;
8. back up an existing desktop file, copy the archived bytes to the desktop,
   and require all three SHA-256 values to match.

No desktop file is replaced before every preceding gate passes. A failed build
leaves existing desktop packages untouched and reports the failed stage.

The required production command must not install or run the generated EXE on the
user's real machine. Lifecycle verification that needs an installed uninstaller
runs only in an isolated CI runner or disposable test environment. Local static
verification covers the final installer and built main executable; the release
workflow remains responsible for full installer, installed executable, and
uninstaller verification when trusted signing is activated.

## Command And Error Contract

The distributable script emits stable stage labels:

- `preflight`
- `brand-sync`
- `build`
- `sign`
- `sign-verify`
- `archive`
- `desktop-copy`

Every failure includes the brand, stage, and affected path without printing
certificate bytes or passwords. Successful output includes the product name,
version, publisher, commit, archive path, delivery path, and SHA-256.

Unsigned development artifacts remain possible under Cargo/Tauri target
directories, but their filenames and logs must identify them as development
outputs. The distributable command never returns an unsigned path as success.

## Workflow Policy

When trusted signing becomes available, stable Windows release jobs set
Authenticode verification to required and remove the optional unsigned fallback.
The Tauri updater signature is generated only after Authenticode signing so it
binds the final published bytes. GitHub Release and mirror uploads must match the
verified digest.

Until a provider is available, the project may continue normal compile and
package smoke tests, but it must not generate a desktop deliverable through the
new distributable command. Documentation continues to state that existing
historical Windows packages are unsigned.

## Testing

1. PowerShell unit-style tests cover missing certificate, invalid signature,
   wrong publisher, wrong product, hash mismatch, archive failure, desktop
   backup, and successful staged delivery using disposable fixtures.
2. Tests prove a failed preflight or signing gate never changes an existing
   desktop file.
3. Existing optional signing tests continue to cover developer and pull-request
   builds.
4. CI exercises the required path with a mock signing adapter only to validate
   control flow; it must not claim a trusted production signature.
5. Real-provider activation requires a separate run that verifies actual signed
   x64 and ARM64 artifacts and all PE layers named in the signing policy.

## Out Of Scope

- generating, purchasing, or bypassing the need for a trusted certificate;
- disabling Smart App Control, SmartScreen, Defender, or enterprise policy;
- installing a self-signed root certificate on user computers;
- force-signing OpenAI or third-party binaries as project-owned code;
- changing the shared Tauri application identifier to support side-by-side
  installation.
