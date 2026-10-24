import { spawn } from "node:child_process";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

const codexBin = process.env.CODEX_BIN || "codex";
let prefixArgs = [];
try {
  prefixArgs = JSON.parse(process.env.CODEX_BIN_PREFIX_ARGS_JSON || "[]");
  if (!Array.isArray(prefixArgs) || prefixArgs.some(value => typeof value !== "string")) throw new Error("not a string array");
} catch (error) {
  console.error(`Invalid CODEX_BIN_PREFIX_ARGS_JSON: ${error.message}`);
  process.exit(1);
}
let recoveredThreadIdSource = process.env.CODEX_RECOVERED_THREAD_IDS || "";
if (!recoveredThreadIdSource) {
  try {
    const codexHome = process.env.CODEX_HOME || path.join(homedir(), ".codex");
    const reports = readdirSync(codexHome)
      .filter(name => /^history-recovery-report-\d{8}-\d{6}\.json$/.test(name))
      .map(name => ({ name, modified: statSync(path.join(codexHome, name)).mtimeMs }))
      .sort((left, right) => right.modified - left.modified);
    if (reports.length > 0) {
      const report = JSON.parse(readFileSync(path.join(codexHome, reports[0].name), "utf8"));
      recoveredThreadIdSource = (report.partial_recoveries || []).map(item => item?.id).join(",");
    }
  } catch (error) {
    console.error(`Could not load the latest history recovery report: ${error.message}`);
    process.exit(1);
  }
}
const recoveredIds = [...new Set(recoveredThreadIdSource
  .split(/[\s,;]+/).map(value => value.trim().toLowerCase())
  .filter(value => /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value)))];
if (recoveredIds.length === 0) {
  console.error("CODEX_RECOVERED_THREAD_IDS did not contain a valid thread ID.");
  process.exit(1);
}

const child = spawn(codexBin, [...prefixArgs, "app-server", "--listen", "stdio://"], { stdio: ["pipe", "pipe", "pipe"] });
let stderr = "";
child.stderr.on("data", chunk => { stderr = (stderr + chunk.toString("utf8")).slice(-8000); });
const send = message => child.stdin.write(`${JSON.stringify(message)}\n`);
const timeout = setTimeout(() => fail(`Timed out.\n${stderr}`), 120_000);
let nextId = 10;
const pages = new Map();
const listed = new Set();
const completedLists = new Set();
const reads = new Map();

function fail(message) {
  clearTimeout(timeout);
  console.error(message);
  child.kill();
  process.exit(1);
}
function requestPage(archived, cursor = null) {
  const id = nextId++;
  pages.set(id, archived);
  send({ jsonrpc: "2.0", id, method: "thread/list", params: {
    archived, cursor, limit: 200, modelProviders: null, sortKey: "updated_at", useStateDbOnly: true,
  }});
}
function startReads() {
  if (completedLists.size !== 2) return;
  const missing = recoveredIds.filter(id => !listed.has(id));
  if (missing.length) return fail(`Recovered threads are missing from thread/list: ${missing.join(", ")}`);
  for (const threadId of recoveredIds) {
    const id = nextId++;
    reads.set(id, threadId);
    send({ jsonrpc: "2.0", id, method: "thread/read", params: { threadId, includeTurns: true } });
  }
}

createInterface({ input: child.stdout }).on("line", line => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  if (message.id === 1) {
    if (message.error) return fail(JSON.stringify(message.error));
    send({ jsonrpc: "2.0", method: "initialized", params: {} });
    requestPage(false);
    requestPage(true);
    return;
  }
  if (pages.has(message.id)) {
    const archived = pages.get(message.id);
    pages.delete(message.id);
    if (message.error) return fail(JSON.stringify(message.error));
    for (const thread of message.result?.data || []) if (thread?.id) listed.add(String(thread.id).toLowerCase());
    if (message.result?.nextCursor) requestPage(archived, message.result.nextCursor);
    else { completedLists.add(archived); startReads(); }
    return;
  }
  if (reads.has(message.id)) {
    const expected = reads.get(message.id);
    if (message.error) return fail(`thread/read failed for ${expected}: ${JSON.stringify(message.error)}`);
    const thread = message.result?.thread ?? message.result;
    if (String(thread?.id || "").toLowerCase() !== expected) return fail(`thread/read returned wrong ID for ${expected}`);
    if (!Array.isArray(thread.turns) || thread.turns.length === 0) return fail(`thread/read returned no turns for ${expected}`);
    reads.delete(message.id);
    if (reads.size === 0) {
      clearTimeout(timeout);
      console.log(JSON.stringify({ listed: listed.size, read: recoveredIds.length }));
      child.stdin.end();
      setTimeout(() => child.kill(), 250).unref();
    }
  }
});
child.on("error", error => fail(`Could not start Codex app-server: ${error.message}`));
send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { clientInfo: { name: "cam-history-recovery-smoke", version: "1.0.0" }, capabilities: { experimentalApi: true } } });
