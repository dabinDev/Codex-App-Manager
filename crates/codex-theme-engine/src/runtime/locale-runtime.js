(() => {
  "use strict";

  const nextCatalog = __CAM_LOCALE_CATALOG__;
  const nextStamp = __CAM_LOCALE_STAMP__;
  const existing = window.__camLocaleRuntimeV1;
  if (existing?.version === 1 && typeof existing.install === "function") {
    existing.install(nextCatalog, nextStamp);
    return existing.report();
  }

  const USER_CONTENT_SELECTOR = [
    "[data-message-author-role]",
    "[data-testid*='conversation']",
    "[data-testid*='message']",
    "[data-testid*='task-title']",
    "[data-testid*='thread-title']",
    "[data-testid*='workspace']",
    "[data-testid*='file-label']",
    "[data-testid*='attachment']",
    "[data-testid*='tool-output']",
    ".task-title",
    ".thread-title",
    ".workspace-name",
    ".file-label",
    ".attachment-name",
    ".tool-output",
  ].join(",");
  const CONTENT_PROTECTED_SELECTOR = [
    USER_CONTENT_SELECTOR,
    "pre",
    "code",
    ".diff",
    "[class*='diff-']",
    ".terminal",
    "[class*='terminal']",
    ".monaco-editor",
    "[class*='editor']",
    "input",
    "textarea",
    "[contenteditable='true']",
    "[contenteditable='']",
  ].join(",");
  const UI_SURFACE_SELECTOR = [
    "button",
    "[role='button']",
    "[role='dialog']",
    "[role='alertdialog']",
    "[role='menu']",
    "[role='menuitem']",
    "[role='menubar']",
    "[role='tab']",
    "[role='tablist']",
    "[role='toolbar']",
    "[role='search']",
    "[role='status']",
    "[role='alert']",
    "label",
    "input[placeholder]",
    "textarea[placeholder]",
    "[data-testid*='settings']",
    "[data-testid*='preferences']",
    "[data-testid*='command-menu']",
  ].join(",");
  const TRANSLATABLE_ATTRIBUTES = ["aria-label", "title", "placeholder"];

  let catalog = Object.freeze({ ...nextCatalog });
  let stamp = nextStamp;
  let scheduled = false;
  let observerActive = true;
  const translatedText = new Map();
  const translatedAttributes = new Map();

  function userContentElement(element) {
    return Boolean(element?.closest?.(USER_CONTENT_SELECTOR));
  }

  function protectedContentElement(element) {
    return Boolean(element?.closest?.(CONTENT_PROTECTED_SELECTOR));
  }

  function translatableElement(element) {
    return !userContentElement(element) && Boolean(element?.closest?.(UI_SURFACE_SELECTOR));
  }

  function visibleElement(element) {
    if (!element?.isConnected) return false;
    for (let current = element; current; current = current.parentElement) {
      if (current.hidden || current.getAttribute("aria-hidden") === "true") return false;
      const style = current.getAttribute("style") || "";
      if (/display\s*:\s*none|visibility\s*:\s*hidden/i.test(style)) return false;
      const computed = window.getComputedStyle?.(current);
      if (
        computed &&
        (computed.display === "none" ||
          computed.visibility === "hidden" ||
          computed.visibility === "collapse" ||
          computed.opacity === "0" ||
          computed.contentVisibility === "hidden")
      ) {
        return false;
      }
    }
    return true;
  }

  function translateText(node) {
    const parent = node.parentElement;
    if (!parent || protectedContentElement(parent) || !translatableElement(parent)) return;
    const original = node.nodeValue;
    const replacement = catalog[original];
    if (typeof replacement !== "string" || replacement === original) return;
    node.nodeValue = replacement;
    translatedText.set(node, { original, replacement });
  }

  function translateAttributes(element) {
    if (!translatableElement(element)) return;
    for (const name of TRANSLATABLE_ATTRIBUTES) {
      const value = element.getAttribute(name);
      const replacement = value == null ? undefined : catalog[value];
      if (typeof replacement !== "string" || replacement === value) continue;
      element.setAttribute(name, replacement);
      let attributes = translatedAttributes.get(element);
      if (!attributes) {
        attributes = new Map();
        translatedAttributes.set(element, attributes);
      }
      attributes.set(name, { original: value, replacement });
    }
  }

  function restoreTranslations() {
    for (const [node, translation] of translatedText) {
      if (node.nodeValue === translation.replacement) node.nodeValue = translation.original;
    }
    translatedText.clear();
    for (const [element, attributes] of translatedAttributes) {
      for (const [name, translation] of attributes) {
        if (element.getAttribute(name) === translation.replacement) {
          element.setAttribute(name, translation.original);
        }
      }
    }
    translatedAttributes.clear();
  }

  function scan(root) {
    if (!root) return;
    if (root.nodeType === Node.TEXT_NODE) {
      translateText(root);
      return;
    }
    if (root.nodeType !== Node.ELEMENT_NODE && root.nodeType !== Node.DOCUMENT_NODE) return;
    if (root.nodeType === Node.ELEMENT_NODE) translateAttributes(root);

    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node) {
      if (node.nodeType === Node.TEXT_NODE) translateText(node);
      else translateAttributes(node);
      node = walker.nextNode();
    }
  }

  function translatedNodeCount() {
    const visible = new Set();
    for (const [node, translation] of translatedText) {
      const element = node.parentElement;
      if (node.nodeValue !== translation.replacement || !element?.isConnected) {
        translatedText.delete(node);
      } else if (visibleElement(element)) {
        visible.add(element);
      }
    }
    for (const [element, attributes] of translatedAttributes) {
      for (const [name, translation] of attributes) {
        if (element.getAttribute(name) !== translation.replacement) attributes.delete(name);
      }
      if (!element.isConnected || attributes.size === 0) {
        translatedAttributes.delete(element);
      } else if (visibleElement(element)) {
        visible.add(element);
      }
    }
    return visible.size;
  }

  function flush(records) {
    scheduled = false;
    if (!observerActive) return;
    for (const record of records) {
      if (record.type === "attributes") translateAttributes(record.target);
      if (record.type === "characterData") scan(record.target);
      for (const node of record.addedNodes || []) scan(node);
    }
  }

  let pendingRecords = [];
  const observer = new MutationObserver((records) => {
    pendingRecords.push(...records);
    if (scheduled) return;
    scheduled = true;
    queueMicrotask(() => {
      const recordsToFlush = pendingRecords;
      pendingRecords = [];
      flush(recordsToFlush);
    });
  });

  const api = {
    version: 1,
    install(next, nextRuntimeStamp) {
      restoreTranslations();
      catalog = Object.freeze({ ...next });
      stamp = nextRuntimeStamp;
      document.documentElement.dataset.camLocale = stamp;
      scan(document.documentElement);
      return this.report();
    },
    remove() {
      observer.disconnect();
      observerActive = false;
      pendingRecords = [];
      restoreTranslations();
      document.documentElement.removeAttribute("data-cam-locale");
      delete window.__camLocaleRuntimeV1;
    },
    report() {
      return {
        version: 1,
        stamp,
        observerActive,
        catalogEntries: Object.keys(catalog).length,
        translatedNodes: translatedNodeCount(),
      };
    },
  };

  window.__camLocaleRuntimeV1 = api;
  api.install(catalog, stamp);
  observer.observe(document.documentElement, {
    subtree: true,
    childList: true,
    characterData: true,
    attributes: true,
    attributeFilter: TRANSLATABLE_ATTRIBUTES,
  });
  return api.report();
})();
