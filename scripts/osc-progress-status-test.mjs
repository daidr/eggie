#!/usr/bin/env node

// Run inside an Eggie terminal to inspect the shared icon indicator and OSC 7501 sidebar labels.
// Usage: node scripts/osc-progress-status-test.mjs [--manual | --step-ms=1600 | --dry-run]

import { createInterface } from "node:readline/promises";

const ESC = "\x1b";
const BEL = "\x07";
const ST = `${ESC}\\`;
const args = process.argv.slice(2);
const manual = args.includes("--manual");
const dryRun = args.includes("--dry-run");
const durationArg = args.find((arg) => arg.startsWith("--step-ms="));
const stepMs = durationArg ? Number(durationArg.slice("--step-ms=".length)) : 1600;
let active = false;

function write(text) {
  process.stdout.write(text);
}

function osc(code, body, terminator = ST) {
  write(`${ESC}]${code};${body}${terminator}`);
}

function progress(state, percent, terminator = ST) {
  osc(9, `4;${state}${percent === undefined ? "" : `;${percent}`}`, terminator);
}

function status(fields, terminator = ST) {
  const pairs = Object.entries(fields).map(([key, value]) => {
    const encoded = key === "title" || key === "msg"
      ? Buffer.from(String(value), "utf8").toString("base64")
      : String(value);
    return `${key}=${encoded}`;
  });
  osc(7501, pairs.join(":"), terminator);
}

function clear() {
  progress(0);
  status({ state: "clear" });
}

function query7501(terminator) {
  return new Promise((resolve, reject) => {
    const input = process.stdin;
    const wasRaw = Boolean(input.isRaw);
    const expected = `${ESC}]7501;?${terminator}`;
    let received = "";
    let finished = false;
    let timer;
    const finish = (error) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      input.off("data", onData);
      input.setRawMode(wasRaw);
      if (!wasRaw) input.pause();
      if (error) reject(error);
      else resolve();
    };
    const onData = (chunk) => {
      if (chunk.includes(3)) {
        const error = new Error("Interrupted");
        error.code = "ABORT_ERR";
        return finish(error);
      }
      received = (received + chunk.toString("latin1")).slice(-128);
      if (received.includes(expected)) finish();
    };
    try {
      input.setRawMode(true);
      input.resume();
      input.on("data", onData);
      timer = setTimeout(() => finish(new Error("OSC 7501 feature query timed out")), 2000);
      osc(7501, "?", terminator);
    } catch (error) {
      finish(error);
    }
  });
}

const title = "Arch review";
const base = { app: "claude-code", title };
const scenes = [
  ["OSC 9;4: 0%", "右上角显示空进度环，主体图标保持不变", () => progress(1, 0)],
  ["OSC 9;4: 25%", "进度环填充四分之一", () => progress(1, 25)],
  ["OSC 9;4: 50%", "进度环填充一半", () => progress(1, 50)],
  ["OSC 9;4: 75%", "进度环填充四分之三", () => progress(1, 75)],
  ["OSC 9;4: 100%", "进度环短暂完成后自动消失", () => progress(1, 100)],
  ["OSC 9;4: indeterminate", "右上角进度环持续旋转", () => progress(3)],
  ["OSC 9;4: paused (BEL)", "右上角显示暂停色的进度环", () => progress(4, 45, BEL)],
  ["OSC 9;4: error", "右上角显示错误色的进度环", () => progress(2, 65)],
  ["OSC 9;4: clear", "进度环消失", () => progress(0)],
  ["OSC 7501: idle", "侧边栏两行，图标无角标", () => status({ state: "idle", ...base, msg: "Ready" })],
  ["OSC 7501: working", "侧边栏两行，图标右上角持续旋转", () => status({ state: "working", ...base, msg: "Reviewing changes" })],
  ["OSC 7501: working 40%", "图标右上角显示 40% 进度环", () => status({ state: "working", ...base, progress: 40, msg: "Reviewing changes" })],
  ["OSC 7501: blocked permission", "标题显示 Blocked，图标右上角出现蓝点", () => status({ state: "blocked", kind: "permission", ...base, msg: "Approve file edit?" })],
  ["OSC 7501: invalid base64", "无效报告被拒绝，原有蓝点和 Blocked 状态保持不变", () => osc(7501, "state=working:msg=QQ=")],
  ["OSC 7501: invalid id", "无效 ID 被拒绝，原有状态保持不变", () => osc(7501, "state=working:id=review//check")],
  ["OSC 7501: blocked question", "蓝点继续显示，第二行更换为问题", () => status({ state: "blocked", kind: "question", ...base, msg: "Choose an option" })],
  ["OSC 7501: blocked auth (BEL)", "蓝点继续显示，第二行更换为认证提示", () => status({ state: "blocked", kind: "auth", ...base, msg: "Sign in to continue" }, BEL)],
  ["OSC 7501: done", "标题显示 Done，图标右上角为蓝点", () => status({ state: "done", ...base, msg: "Review complete" })],
  ["OSC 7501: error", "标题显示 Error，图标右上角为红点", () => status({ state: "error", ...base, msg: "Review failed" })],
  ["OSC 7501: UTF-8", "标题与消息正确显示中文", () => status({ state: "working", app: "claude-code", title: "架构评审", msg: "正在检查变更" })],
  ["OSC 7501: child record", "子记录优先显示，并从根记录继承 Claude 图标", () => {
    status({ state: "working", ...base, msg: "Reviewing" });
    status({ state: "blocked", id: "review/check", kind: "question", title: "Dependency review", msg: "Need an answer" });
  }],
  ["OSC 7501: clear child", "清除子记录后恢复根记录的 working", () => status({ state: "clear", id: "review" })],
  ["OSC 7501 + OSC 9;4", "只显示 7501 的蓝点，不叠加第二个进度环", () => {
    progress(3);
    status({ state: "blocked", kind: "permission", ...base, msg: "Approve change?" });
  }],
  ["OSC 7501: clear while OSC 9;4 remains", "蓝点消失后显示 OSC 9;4 旋转环", () => status({ state: "clear" })],
  ["OSC 9;4: final clear", "恢复纯净图标和单行侧边栏 item", clear],
];

async function pause() {
  if (!manual) {
    await new Promise((resolve) => setTimeout(resolve, stepMs));
    return;
  }
  const input = createInterface({ input: process.stdin, output: process.stdout });
  try {
    await input.question("  按回车继续…");
  } finally {
    input.close();
  }
}

async function main() {
  if (args.includes("--help") || args.includes("-h")) {
    write("Usage: node scripts/osc-progress-status-test.mjs [--manual | --step-ms=1600 | --dry-run]\n");
    return;
  }
  if (!Number.isInteger(stepMs) || stepMs < 250 || stepMs > 30000) {
    throw new Error("--step-ms must be an integer from 250 to 30000");
  }
  if (dryRun) {
    for (const [name, expectation] of scenes) write(`${name}: ${expectation}\n`);
    return;
  }
  if (!process.stdin.isTTY || !process.stdout.isTTY || !process.stdin.setRawMode) {
    throw new Error("Run this script in an interactive Eggie terminal");
  }
  if (process.env.TERM_PROGRAM !== "Eggie") {
    write(`Warning: TERM_PROGRAM=${process.env.TERM_PROGRAM ?? "(unset)"}; visual checks target Eggie.\n`);
  }

  active = true;
  write("Checking OSC 7501 feature replies (ST and BEL)…\n");
  await query7501(ST);
  await query7501(BEL);
  write("Both queries passed. Watch the tab icon and sidebar terminal item.\n");
  try {
    for (const [name, expectation, send] of scenes) {
      write(`\n[${name}] ${expectation}\n`);
      send();
      await pause();
    }
  } finally {
    clear();
    active = false;
  }
  write("\nComplete. OSC 9;4 and OSC 7501 have been cleared.\n");
}

process.once("SIGINT", () => {
  if (active) clear();
  process.exit(130);
});

main().catch((error) => {
  if (active) {
    clear();
    active = false;
  }
  if (error?.code === "ABORT_ERR" || error?.name === "AbortError") {
    write("\nInterrupted. OSC 9;4 and OSC 7501 have been cleared.\n");
    process.exitCode = 130;
    return;
  }
  console.error(error);
  process.exitCode = 1;
});
