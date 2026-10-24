import { readFile } from "node:fs/promises";
import { join } from "node:path";

import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";

const repoRoot = join(import.meta.dirname, "..");
const runtimePath = join(
  repoRoot,
  "crates",
  "codex-theme-engine",
  "src",
  "runtime",
  "locale-runtime.js",
);

async function installRuntime(body) {
  const template = await readFile(runtimePath, "utf8");
  const dom = new JSDOM(body, {
    runScripts: "outside-only",
    url: "https://codex.local/",
  });
  const payload = template
    .replace("__CAM_LOCALE_CATALOG__", JSON.stringify({
      Settings: "设置",
      "Open settings": "打开设置",
      Confirm: "确认",
      Search: "搜索",
    }))
    .replace("__CAM_LOCALE_STAMP__", JSON.stringify("test-stamp"));
  dom.window.eval(payload);
  await new Promise((resolve) => dom.window.setTimeout(resolve, 0));
  return dom;
}

describe("Codex renderer locale runtime", () => {
  it("translates exact UI text and accessible attributes", async () => {
    const dom = await installRuntime(`
      <main>
        <button title="Open settings" aria-label="Open settings">Settings</button>
        <div role="dialog"><span>Confirm</span></div>
      </main>
    `);

    const button = dom.window.document.querySelector("button");
    expect(button.textContent).toBe("设置");
    expect(button.title).toBe("打开设置");
    expect(button.getAttribute("aria-label")).toBe("打开设置");
    expect(dom.window.document.querySelector('[role="dialog"] span').textContent).toBe("确认");

    const report = dom.window.__camLocaleRuntimeV1.report();
    expect(report).toMatchObject({
      stamp: "test-stamp",
      observerActive: true,
      catalogEntries: 4,
      translatedNodes: 2,
    });
    expect(dom.window.document.documentElement.dataset.camLocale).toBe("test-stamp");
  });

  it("never translates conversation, code, terminal, editor, or editable content", async () => {
    const dom = await installRuntime(`
      <section data-message-author-role="user">Settings</section>
      <section data-message-author-role="assistant"><span>Confirm</span></section>
      <pre>Settings</pre><code>Confirm</code>
      <div class="diff">Settings</div><div class="terminal">Confirm</div>
      <div class="monaco-editor">Settings</div>
      <div role="search">
        <input aria-label="Search" placeholder="Search" value="Settings">
        <textarea aria-label="Search" placeholder="Search">Confirm</textarea>
      </div>
      <div contenteditable="true">Settings</div>
    `);

    expect(dom.window.document.body.textContent).toContain("Settings");
    expect(dom.window.document.body.textContent).toContain("Confirm");
    const input = dom.window.document.querySelector("input");
    const textarea = dom.window.document.querySelector("textarea");
    expect(input.value).toBe("Settings");
    expect(textarea.value).toBe("Confirm");
    expect(input.placeholder).toBe("搜索");
    expect(input.getAttribute("aria-label")).toBe("搜索");
    expect(textarea.placeholder).toBe("搜索");
    expect(textarea.getAttribute("aria-label")).toBe("搜索");
    expect(dom.window.__camLocaleRuntimeV1.report().translatedNodes).toBe(2);
  });

  it("observes new UI and reinjects without creating another observer", async () => {
    const dom = await installRuntime("<main><span>Settings</span></main>");
    const first = dom.window.__camLocaleRuntimeV1;
    const dialog = dom.window.document.createElement("div");
    dialog.setAttribute("role", "dialog");
    dialog.textContent = "Confirm";
    dom.window.document.body.append(dialog);
    await new Promise((resolve) => dom.window.setTimeout(resolve, 0));

    expect(dialog.textContent).toBe("确认");
    dom.window.eval(
      (await readFile(runtimePath, "utf8"))
        .replace("__CAM_LOCALE_CATALOG__", JSON.stringify({ Settings: "设定", Confirm: "确定" }))
        .replace("__CAM_LOCALE_STAMP__", JSON.stringify("second-stamp")),
    );
    await new Promise((resolve) => dom.window.setTimeout(resolve, 0));

    expect(dom.window.__camLocaleRuntimeV1).toBe(first);
    expect(dom.window.__camLocaleRuntimeV1.report().stamp).toBe("second-stamp");
  });

  it("translates cataloged in-place text node updates", async () => {
    const dom = await installRuntime('<main><div role="status"><span>Waiting</span></div></main>');
    const text = dom.window.document.querySelector("span").firstChild;

    text.nodeValue = "Confirm";
    await new Promise((resolve) => dom.window.setTimeout(resolve, 0));

    expect(text.nodeValue).toBe("确认");
  });

  it("preserves user-owned labels outside messages", async () => {
    const dom = await installRuntime(`
      <button data-testid="task-title">Settings</button>
      <div class="workspace-name">Confirm</div>
      <span data-testid="file-label">Settings</span>
      <span class="attachment-name">Confirm</span>
      <section data-testid="tool-output">Settings</section>
    `);

    expect(dom.window.document.body.textContent).toContain("Settings");
    expect(dom.window.document.body.textContent).toContain("Confirm");
    expect(dom.window.__camLocaleRuntimeV1.report().translatedNodes).toBe(0);
  });

  it("counts only currently translated visible UI", async () => {
    const dom = await installRuntime(`
      <style>.hidden-by-class { display: none; }</style>
      <div role="dialog" class="hidden-by-class"><span>Confirm</span></div>
      <button aria-label="Open settings">Settings</button>
    `);
    const button = dom.window.document.querySelector("button");

    expect(dom.window.__camLocaleRuntimeV1.report().translatedNodes).toBe(1);

    button.firstChild.nodeValue = "Unrelated task name";
    button.setAttribute("aria-label", "Unrelated task name");
    await new Promise((resolve) => dom.window.setTimeout(resolve, 0));
    expect(dom.window.__camLocaleRuntimeV1.report().translatedNodes).toBe(0);

    button.setAttribute("aria-label", "Open settings");
    await new Promise((resolve) => dom.window.setTimeout(resolve, 0));
    button.setAttribute("aria-label", "Open settings");
    await new Promise((resolve) => dom.window.setTimeout(resolve, 0));
    expect(dom.window.__camLocaleRuntimeV1.report().translatedNodes).toBe(1);
  });

  it("restores runtime-owned text and attributes when removed", async () => {
    const dom = await installRuntime(`
      <main>
        <button title="Open settings" aria-label="Open settings">Settings</button>
        <div role="dialog"><span>Confirm</span></div>
      </main>
    `);
    const button = dom.window.document.querySelector("button");
    const dialogText = dom.window.document.querySelector('[role="dialog"] span');

    button.setAttribute("title", "Changed by Codex");
    dom.window.__camLocaleRuntimeV1.remove();

    expect(button.textContent).toBe("Settings");
    expect(button.getAttribute("aria-label")).toBe("Open settings");
    expect(button.getAttribute("title")).toBe("Changed by Codex");
    expect(dialogText.textContent).toBe("Confirm");
    expect(dom.window.document.documentElement.dataset.camLocale).toBeUndefined();
    expect(dom.window.__camLocaleRuntimeV1).toBeUndefined();
  });
});

describe("locale daemon directive contract", () => {
  it("mutates theme and locale fields atomically", async () => {
    const source = await readFile(
      join(repoRoot, "src-tauri", "src", "app", "codex_theme.rs"),
      "utf8",
    );
    const start = source.indexOf("async fn set_theme_directive");
    const end = source.indexOf("async fn daemon_status", start);
    const setters = source.slice(start, end);

    expect(setters.match(/send_modify/g)).toHaveLength(2);
    expect(setters).not.toContain("let current = handle.borrow()");
  });
});
