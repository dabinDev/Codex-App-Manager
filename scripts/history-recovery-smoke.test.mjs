import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { describe, expect, it } from "vitest";

describe("history recovery desktop smoke", () => {
  it("follows normal and archived cursors before reading every recovered thread", () => {
    const root = mkdtempSync(path.join(tmpdir(), "cam-history-smoke-"));
    try {
      const server = path.join(root, "fake-server.cjs");
      const log = path.join(root, "requests.jsonl");
      const normalId = "019b5555-1111-7222-8333-444444444444";
      const archivedId = "019b6666-1111-7222-8333-444444444444";
      writeFileSync(path.join(root, "history-recovery-report-20260806-120000.json"), JSON.stringify({
        partial_recoveries: [{ id: normalId }, { id: archivedId }],
      }), "utf8");
      writeFileSync(server, `
const fs = require("node:fs");
const readline = require("node:readline");
readline.createInterface({ input: process.stdin }).on("line", line => {
  const request = JSON.parse(line);
  fs.appendFileSync(process.env.REQUEST_LOG, line + "\\n");
  let result = {};
  if (request.method === "thread/list") {
    const archived = request.params.archived === true;
    if (request.params.cursor === null) result = { data: [{ id: "019b7777-1111-7222-8333-444444444444" }], nextCursor: archived ? "archived-2" : "normal-2" };
    else result = { data: [{ id: archived ? process.env.ARCHIVED_ID : process.env.NORMAL_ID }], nextCursor: null };
  } else if (request.method === "thread/read") {
    result = { thread: { id: request.params.threadId, turns: [{ id: "turn" }] } };
  }
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\\n");
});`, "utf8");
      const result = spawnSync(process.execPath, ["scripts/history-recovery-smoke.mjs"], {
        cwd: process.cwd(), encoding: "utf8", timeout: 30_000,
        env: { ...process.env, CODEX_HOME: root, CODEX_BIN: process.execPath, CODEX_BIN_PREFIX_ARGS_JSON: JSON.stringify([server]), CODEX_RECOVERED_THREAD_IDS: "", REQUEST_LOG: log, NORMAL_ID: normalId, ARCHIVED_ID: archivedId },
      });
      expect(result.status, result.stderr).toBe(0);
      const requests = readFileSync(log, "utf8").trim().split(/\r?\n/).map(JSON.parse);
      expect(requests.some(r => r.method === "thread/list" && r.params.cursor === "normal-2")).toBe(true);
      expect(requests.some(r => r.method === "thread/list" && r.params.cursor === "archived-2")).toBe(true);
      expect(requests.filter(r => r.method === "thread/read" && r.params.includeTurns === true).map(r => r.params.threadId).sort()).toEqual([archivedId, normalId].sort());
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
