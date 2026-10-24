import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * Window focus → silently re-detect the local install (milliseconds, no
 * network). If the install identity (version/build + path) no longer matches
 * the snapshot on screen — Codex was updated / downgraded / removed / moved
 * out-of-band while we weren't looking — run the full re-check so the card
 * corrects itself instead of waiting to fail the perform-time guard.
 *
 * Shared by both platform homes (previously only the mac home had it). The
 * listener installs ONCE and reads the latest callbacks via refs, so it never
 * tears down on a state change; non-Tauri (web preview / tests) is a no-op.
 */
export function useFocusRecheck<S extends { installed: unknown }>(opts: {
  /** Local, network-free status probe (macStatus / winStatus). */
  fetchStatus: () => Promise<S>;
  /** Apply the fresh status (setStatus / setStatusLoaded / clear failed). */
  onStatus: (status: S) => void;
  /** Have we planned at least once? Gate on this, not on a non-null install,
   *  or the none→installed transition is missed. */
  hasChecked: () => boolean;
  /** Stable identity of the last CHECKED install (build/version + path). */
  checkedIdentity: () => string | null;
  /** Stable identity of a freshly-probed install. */
  identityOf: (status: S) => string | null;
  /** True while an operation is in flight — skip the probe. */
  isBusy: () => boolean;
  /** The identity drifted: drop any open confirm sheet + re-check. */
  onIdentityChanged: () => void;
}) {
  // Stash the latest callbacks so the focus listener installs once and never
  // re-subscribes — mirroring the original ref-driven effect.
  const ref = useRef(opts);
  ref.current = opts;

  useEffect(() => {
    let last = 0;
    let disposed = false;
    let un: (() => void) | undefined;
    void (async () => {
      try {
        const unlisten = await listen("tauri://focus", () => {
          const now = Date.now();
          if (disposed || ref.current.isBusy() || now - last < 3000) return;
          last = now;
          void (async () => {
            try {
              const st = await ref.current.fetchStatus();
              if (disposed) return;
              ref.current.onStatus(st);
              if (
                ref.current.hasChecked() &&
                ref.current.checkedIdentity() !== ref.current.identityOf(st)
              ) {
                ref.current.onIdentityChanged();
              }
            } catch {
              // Transient/unsupported — the next explicit check will surface it.
            }
          })();
        });
        if (disposed) unlisten();
        else un = unlisten;
      } catch {
        // Non-Tauri (web preview): no event bus — nothing to clean up.
      }
    })();
    return () => {
      disposed = true;
      un?.();
    };
  }, []);
}

/** Stable identity string for an install: `null` when absent, else the pieces
 *  whose change should force a re-plan (version/build + path). Accepts either
 *  platform's installed shape — mac keys on `build`, Windows on `version`.
 *  `normalizePath` folds cosmetic path differences (Windows returns the same
 *  root with mixed casing / separators); mac omits it since its paths are
 *  case-sensitive, so a raw compare is correct there. */
export function installIdentity(
  installed: { path: string; build?: number; version?: string } | null | undefined,
  normalizePath: (path: string) => string = (path) => path,
): string | null {
  if (!installed) return null;
  const stamp = installed.build != null ? String(installed.build) : installed.version ?? "";
  return `${stamp}|${normalizePath(installed.path)}`;
}
