import { act, renderHook } from "@testing-library/react";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { normalizePath } from "../paths";
import { installIdentity, useFocusRecheck } from "./useFocusRecheck";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

function focusOptions() {
  return {
    fetchStatus: vi.fn(async () => ({ installed: null })),
    onStatus: vi.fn(),
    hasChecked: () => true,
    checkedIdentity: () => "old",
    identityOf: () => null,
    isBusy: vi.fn(() => false),
    onIdentityChanged: vi.fn(),
  };
}

describe("useFocusRecheck lifecycle", () => {
  beforeEach(() => vi.mocked(listen).mockReset());

  it("removes a listener whose registration finishes after unmount", async () => {
    const registration = deferred<() => void>();
    const dispose = vi.fn();
    vi.mocked(listen).mockReturnValueOnce(registration.promise);
    const { unmount } = renderHook(() => useFocusRecheck(focusOptions()));

    unmount();
    await act(async () => registration.resolve(dispose));
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  it("ignores a late status result after unmount", async () => {
    vi.mocked(listen).mockResolvedValue(vi.fn());
    const probe = deferred<{ installed: null }>();
    const options = focusOptions();
    options.fetchStatus.mockReturnValueOnce(probe.promise);
    const { unmount } = renderHook(() => useFocusRecheck(options));
    await act(async () => {});
    const onFocus = vi.mocked(listen).mock.calls[0][1];

    act(() => onFocus({ event: "tauri://focus", id: 1, payload: null }));
    unmount();
    await act(async () => probe.resolve({ installed: null }));

    expect(options.onStatus).not.toHaveBeenCalled();
    expect(options.onIdentityChanged).not.toHaveBeenCalled();
  });
});

describe("installIdentity", () => {
  it("is null for an absent install", () => {
    expect(installIdentity(null)).toBeNull();
    expect(installIdentity(undefined)).toBeNull();
  });

  it("keys mac installs on build + raw (case-sensitive) path", () => {
    const a = installIdentity({ build: 100, path: "/Applications/Codex.app" });
    const b = installIdentity({ build: 100, path: "/Applications/codex.app" });
    // Mac paths are case-sensitive — these are DIFFERENT installs.
    expect(a).not.toBe(b);
    expect(installIdentity({ build: 101, path: "/Applications/Codex.app" })).not.toBe(a);
  });

  it("folds Windows path casing / separators so a cosmetic diff isn't drift", () => {
    const a = installIdentity(
      { version: "1.0.0", path: "C:\\Program Files\\Codex" },
      normalizePath,
    );
    const b = installIdentity(
      { version: "1.0.0", path: "c:/program files/codex/" },
      normalizePath,
    );
    // Same install, different spelling — must be the SAME identity.
    expect(a).toBe(b);
    // A real version change is still drift.
    expect(installIdentity({ version: "2.0.0", path: "C:\\Program Files\\Codex" }, normalizePath)).not.toBe(a);
  });
});
