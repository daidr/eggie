#!/usr/bin/env bun

/**
 * Interactive coverage tool for every OSC command currently handled by Eggie.
 *
 * Usage:
 *   bun scripts/osc-test.ts --list
 *   bun scripts/osc-test.ts
 *   bun scripts/osc-test.ts --only=colors,progress
 *   bun scripts/osc-test.ts --yes
 *
 * Run this inside Eggie. Query tests validate replies automatically. Tests that
 * affect macOS (clipboard, notifications, files, URLs, focus) ask first unless
 * --yes is supplied.
 */

import { randomBytes } from "node:crypto";
import { hostname } from "node:os";
import { createInterface } from "node:readline/promises";
import { pathToFileURL } from "node:url";
import { deflateSync } from "node:zlib";

const ESC = "\x1b";
const ST = `${ESC}\\`;
const CSI = `${ESC}[`;
const OSC = `${ESC}]`;
const TEST_CLIPBOARD_TEXT = `Eggie OSC test ${new Date().toISOString()}`;
const TEST_PNG_WIDTH = 72;
const TEST_PNG_HEIGHT = 36;

type OscFrame = { code: number; payload: string; raw: string };
type TestCase = {
  id: string;
  title: string;
  codes: string;
  interactive?: boolean;
  run: () => Promise<void>;
};

class SkipTest extends Error {}

const argv = process.argv.slice(2);
const yes = argv.includes("--yes");
const listOnly = argv.includes("--list");
const dryRun = argv.includes("--dry-run");
const selected = (() => {
  const inline = argv.find((arg) => arg.startsWith("--only="))?.slice("--only=".length);
  const index = argv.indexOf("--only");
  const value = inline ?? (index >= 0 ? argv[index + 1] : undefined);
  return value ? new Set(value.split(",").map((item) => item.trim()).filter(Boolean)) : null;
})();

function write(value: string | Uint8Array): void {
  process.stdout.write(value);
}

function osc(code: number, payload: string, terminator = ST): void {
  write(`${OSC}${code};${payload}${terminator}`);
}

function b64(value: string | Uint8Array): string {
  return Buffer.from(value).toString("base64");
}

function crc32(value: Uint8Array): number {
  let crc = 0xffff_ffff;
  for (const byte of value) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ (0xedb8_8320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffff_ffff) >>> 0;
}

function pngChunk(type: string, data: Uint8Array): Buffer {
  const typeBytes = Buffer.from(type, "ascii");
  const chunk = Buffer.allocUnsafe(12 + data.length);
  chunk.writeUInt32BE(data.length, 0);
  typeBytes.copy(chunk, 4);
  Buffer.from(data).copy(chunk, 8);
  chunk.writeUInt32BE(crc32(chunk.subarray(4, 8 + data.length)), 8 + data.length);
  return chunk;
}

function createTestPng(width: number, height: number): Buffer {
  const scanlines = Buffer.alloc((width * 4 + 1) * height);
  const palette = [
    [255, 52, 96],
    [255, 190, 36],
    [0, 190, 218],
    [126, 87, 235],
  ];
  for (let y = 0; y < height; y++) {
    const rowStart = y * (width * 4 + 1);
    scanlines[rowStart] = 0; // PNG filter: None.
    for (let x = 0; x < width; x++) {
      const offset = rowStart + 1 + x * 4;
      const border = x < 2 || y < 2 || x >= width - 2 || y >= height - 2;
      const color = border ? [255, 255, 255] : palette[Math.min(3, Math.floor((x * 4) / width))];
      const shade = !border && (Math.floor(x / 6) + Math.floor(y / 6)) % 2 === 1 ? -34 : 0;
      scanlines[offset] = Math.max(0, color[0] + shade);
      scanlines[offset + 1] = Math.max(0, color[1] + shade);
      scanlines[offset + 2] = Math.max(0, color[2] + shade);
      scanlines[offset + 3] = 255;
    }
  }

  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // Bit depth.
  header[9] = 6; // RGBA.
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(scanlines)),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

const TEST_PNG = createTestPng(TEST_PNG_WIDTH, TEST_PNG_HEIGHT);

function createTestTiff(width: number, height: number): Buffer {
  const entryCount = 11;
  const ifdOffset = 8;
  const bitsPerSampleOffset = ifdOffset + 2 + entryCount * 12 + 4;
  const pixelOffset = bitsPerSampleOffset + 8;
  const pixels = Buffer.alloc(width * height * 4);
  const palette = [
    [255, 52, 96],
    [255, 190, 36],
    [0, 190, 218],
    [126, 87, 235],
  ];
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const offset = (y * width + x) * 4;
      const color = palette[Math.min(3, Math.floor((x * 4) / width))];
      pixels[offset] = color[0];
      pixels[offset + 1] = color[1];
      pixels[offset + 2] = color[2];
      pixels[offset + 3] = 255;
    }
  }

  const tiff = Buffer.alloc(pixelOffset + pixels.length);
  tiff.write("II", 0, "ascii");
  tiff.writeUInt16LE(42, 2);
  tiff.writeUInt32LE(ifdOffset, 4);
  tiff.writeUInt16LE(entryCount, ifdOffset);
  let entry = 0;
  const writeEntry = (tag: number, type: number, count: number, value: number): void => {
    const offset = ifdOffset + 2 + entry++ * 12;
    tiff.writeUInt16LE(tag, offset);
    tiff.writeUInt16LE(type, offset + 2);
    tiff.writeUInt32LE(count, offset + 4);
    if (type === 3 && count === 1) tiff.writeUInt16LE(value, offset + 8);
    else tiff.writeUInt32LE(value, offset + 8);
  };
  writeEntry(256, 4, 1, width); // ImageWidth.
  writeEntry(257, 4, 1, height); // ImageLength.
  writeEntry(258, 3, 4, bitsPerSampleOffset); // BitsPerSample.
  writeEntry(259, 3, 1, 1); // Compression: none.
  writeEntry(262, 3, 1, 2); // PhotometricInterpretation: RGB.
  writeEntry(273, 4, 1, pixelOffset); // StripOffsets.
  writeEntry(277, 3, 1, 4); // SamplesPerPixel.
  writeEntry(278, 4, 1, height); // RowsPerStrip.
  writeEntry(279, 4, 1, pixels.length); // StripByteCounts.
  writeEntry(284, 3, 1, 1); // PlanarConfiguration: chunky.
  writeEntry(338, 3, 1, 2); // ExtraSamples: unassociated alpha.
  tiff.writeUInt32LE(0, ifdOffset + 2 + entryCount * 12);
  for (let channel = 0; channel < 4; channel++) {
    tiff.writeUInt16LE(8, bitsPerSampleOffset + channel * 2);
  }
  pixels.copy(tiff, pixelOffset);
  return tiff;
}

const TEST_TIFF = createTestTiff(TEST_PNG_WIDTH, TEST_PNG_HEIGHT);

function fromB64(value: string): string {
  try {
    return Buffer.from(value, "base64").toString("utf8");
  } catch {
    return "<invalid base64>";
  }
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function printable(value: string): string {
  return value.replaceAll(ESC, "\\e").replaceAll("\x07", "\\a");
}

async function ask(question: string, defaultYes = true): Promise<boolean> {
  if (yes) return true;
  if (!process.stdin.isTTY) return false;

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    const suffix = defaultYes ? "[Y/n]" : "[y/N]";
    const answer = (await rl.question(`${question} ${suffix} `)).trim().toLowerCase();
    if (!answer) return defaultYes;
    return answer === "y" || answer === "yes";
  } finally {
    rl.close();
  }
}

function parseFrames(buffer: string): { frames: OscFrame[]; rest: string } {
  const frames: OscFrame[] = [];
  const pattern = /\x1b\](\d+);([\s\S]*?)(?:\x07|\x1b\\|\x9c)/g;
  let consumed = 0;
  for (const match of buffer.matchAll(pattern)) {
    const index = match.index ?? 0;
    const raw = match[0];
    frames.push({ code: Number(match[1]), payload: match[2], raw });
    consumed = index + raw.length;
  }
  // Keep only a small tail when there is no complete response; PTY input can
  // contain unrelated keypresses while a response is pending.
  return { frames, rest: buffer.slice(consumed).slice(-64 * 1024) };
}

async function collectOsc(
  code: number,
  send: () => void,
  done: (frames: OscFrame[]) => boolean = (frames) => frames.length > 0,
  timeoutMs = 1_500,
): Promise<OscFrame[]> {
  if (!process.stdin.isTTY || typeof process.stdin.setRawMode !== "function") {
    throw new Error("OSC reply tests require a TTY");
  }

  return await new Promise<OscFrame[]>((resolve, reject) => {
    const wasRaw = Boolean(process.stdin.isRaw);
    const frames: OscFrame[] = [];
    let buffer = "";
    let settled = false;

    const cleanup = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      process.stdin.off("data", onData);
      process.stdin.setRawMode?.(wasRaw);
      if (!wasRaw) process.stdin.pause();
    };
    const finish = () => {
      cleanup();
      resolve(frames);
    };
    const onData = (chunk: Buffer) => {
      if (chunk.includes(3)) {
        cleanup();
        reject(new Error("interrupted"));
        process.kill(process.pid, "SIGINT");
        return;
      }
      buffer += chunk.toString("latin1");
      const parsed = parseFrames(buffer);
      buffer = parsed.rest;
      frames.push(...parsed.frames.filter((frame) => frame.code === code));
      if (done(frames)) finish();
    };
    const timer = setTimeout(finish, timeoutMs);

    process.stdin.setRawMode(true);
    process.stdin.resume();
    process.stdin.on("data", onData);
    send();
  });
}

async function query(
  code: number,
  payload: string,
  timeoutMs = 1_500,
): Promise<OscFrame> {
  const replies = await collectOsc(code, () => osc(code, payload), undefined, timeoutMs);
  if (!replies[0]) throw new Error(`OSC ${code} query timed out`);
  return replies[0];
}

function assertMatch(frame: OscFrame, pattern: RegExp, description: string): void {
  if (!pattern.test(frame.payload)) {
    throw new Error(`${description}: unexpected reply ${printable(frame.raw)}`);
  }
}

function metadata(payload: string): Map<string, string> {
  return new Map(
    payload
      .split(/[;:]/)
      .map((field) => {
        const separator = field.indexOf("=");
        return separator < 0
          ? ([field, ""] as [string, string])
          : ([field.slice(0, separator), field.slice(separator + 1)] as [string, string]);
      })
      .filter(([key, value]) => Boolean(key && value !== undefined)),
  );
}

function decodedStatus(frame: OscFrame | undefined): string | undefined {
  const encoded = metadata(frame?.payload ?? "").get("st");
  return encoded ? fromB64(encoded) : undefined;
}

async function countdown(message: string, seconds = 3): Promise<void> {
  for (let remaining = seconds; remaining > 0; remaining--) {
    write(`\r${message} ${remaining}s `);
    await sleep(1_000);
  }
  write("\r" + " ".repeat(message.length + 8) + "\r");
}

async function visualPause(message: string): Promise<void> {
  if (yes) {
    await sleep(900);
    return;
  }
  await ask(`${message}，确认后继续。`, true);
}

const tests: TestCase[] = [
  {
    id: "title",
    title: "窗口/标签标题",
    codes: "OSC 0, 1, 2",
    run: async () => {
      osc(0, "OSC 0 — Eggie title test");
      await sleep(300);
      osc(1, "OSC 1 — Eggie icon title test");
      await sleep(300);
      osc(2, "OSC 2 — Eggie title test");
      await visualPause("标签标题应显示 ‘OSC 2 — Eggie title test’");
    },
  },
  {
    id: "cwd",
    title: "当前目录与远端主机信息",
    codes: "OSC 7, 9;9, 1337 CurrentDir/RemoteHost",
    run: async () => {
      const cwd = process.cwd();
      const uri = pathToFileURL(cwd).href;
      osc(7, uri);
      osc(9, `9;${cwd}`);
      osc(1337, `CurrentDir=${cwd}`);
      osc(1337, `RemoteHost=${process.env.USER ?? "user"}@${hostname()}`);
      write(`  reported ${uri}\n`);
    },
  },
  {
    id: "hyperlink",
    title: "超链接",
    codes: "OSC 8",
    run: async () => {
      osc(8, "id=eggie-osc-test;https://example.com/");
      write("Eggie OSC 8 hyperlink (hover/click test)");
      osc(8, ";");
      write("\n");
      await visualPause("链接应可识别，且后续文本不属于链接");
    },
  },
  {
    id: "colors",
    title: "调色板、动态颜色和重置",
    codes: "OSC 4, 10, 11, 12, 104, 110, 111, 112; OSC 1337 SetColors",
    run: async () => {
      for (const [code, payload] of [
        [4, "1;?"],
        [4, "-1;?"],
        [4, "-2;?"],
        [10, "?"],
        [11, "?"],
        [12, "?"],
      ] as const) {
        const frame = await query(code, payload);
        assertMatch(frame, /rgb:[0-9a-f]{2,4}\/[0-9a-f]{2,4}\/[0-9a-f]{2,4}/i, `OSC ${code}`);
        write(`  ${printable(frame.raw)}\n`);
      }

      osc(4, "1;#ff3355");
      write("\x1b[31mANSI red via OSC 4\x1b[0m\n");
      osc(10, "#e8e8e8");
      osc(11, "#18202a");
      osc(12, "#55ccff");
      osc(1337, "SetColors=fg=eeeeee bg=18202a red=ff3355 curbg=55ccff");
      await visualPause("颜色应短暂变化");
      osc(104, "1");
      osc(104, "");
      osc(110, "");
      osc(111, "");
      osc(112, "");
    },
  },
  {
    id: "progress",
    title: "标签和侧边栏环形进度",
    codes: "OSC 9;4",
    run: async () => {
      for (let percent = 1; percent <= 100; percent++) {
        osc(9, `4;1;${percent}`);
        write(`\r  normal ${percent}%`);
        await sleep(50);
      }
      write("\n");
      osc(9, "4;0");
    },
  },
  {
    id: "cursor",
    title: "鼠标指针与文本光标形状",
    codes: "OSC 22, 50; OSC 1337 CursorShape",
    run: async () => {
      for (const shape of ["pointer", "text", "default"]) {
        osc(22, shape);
        await sleep(350);
      }
      for (const shape of ["1", "2", "0"]) {
        osc(50, `CursorShape=${shape}`);
        await sleep(350);
      }
      for (const shape of ["1", "2", "0"]) {
        osc(1337, `CursorShape=${shape}`);
        await sleep(350);
      }
    },
  },
  {
    id: "clipboard52",
    title: "传统剪贴板写入和读取",
    codes: "OSC 52",
    interactive: true,
    run: async () => {
      if (!(await ask("允许脚本用测试文本覆盖系统剪贴板吗？", false))) throw new SkipTest();
      osc(52, `c;${b64(TEST_CLIPBOARD_TEXT)}`);
      write(`  wrote: ${TEST_CLIPBOARD_TEXT}\n`);
      const frames = await collectOsc(52, () => osc(52, "c;?"), undefined, 3_000);
      if (!frames[0]) {
        write("  read timed out (enable OSC clipboard reads in Settings to test it)\n");
        return;
      }
      const encoded = frames[0].payload.split(";").at(-1) ?? "";
      const decoded = fromB64(encoded);
      if (decoded !== TEST_CLIPBOARD_TEXT) throw new Error(`clipboard mismatch: ${decoded}`);
      write("  read-back matched\n");
    },
  },
  {
    id: "shell133",
    title: "Shell 语义标记",
    codes: "OSC 133 L/A/N/P/B/I/C/D",
    run: async () => {
      osc(133, "L");
      osc(133, "A;aid=eggie-test");
      osc(133, "N");
      osc(133, "P;k=i");
      write("eggie-osc-test $ ");
      osc(133, "B");
      osc(133, "I");
      write("printf semantic-output");
      osc(133, "C");
      write("\nsemantic-output\n");
      osc(133, "D;0");
    },
  },
  {
    id: "notifications",
    title: "桌面通知（基础、rxvt、Kitty）",
    codes: "OSC 9, 99, 777",
    interactive: true,
    run: async () => {
      const capability = await query(99, "i=eggie-capabilities:p=?;");
      assertMatch(capability, /p=\?;.*p=title,body,\?,close,alive/, "OSC 99 capabilities");
      write(`  capabilities: ${capability.payload}\n`);

      if (!(await ask("测试 macOS 通知吗？确认后请在倒计时内切换到其他应用。", true))) {
        throw new SkipTest("native notification portion skipped");
      }
      await countdown("Switch away from Eggie:");
      const immediateReplies = await collectOsc(
        99,
        () => {
          osc(9, "Eggie OSC 9 notification");
          osc(777, "notify;Eggie OSC 777;rxvt-style notification body");
          osc(99, `i=eggie-test:p=title:d=0:e=1;${b64("Eggie OSC 99")}`);
          osc(99, `i=eggie-test:p=body:a=focus,report:c=1:e=1;${b64("Multipart Kitty notification")}`);
        },
        (frames) => frames.some((frame) => frame.payload.includes("p=close;untracked")),
        1_500,
      );
      if (immediateReplies.length > 0) {
        write(`  immediate: ${immediateReplies.map((frame) => frame.payload).join(" | ")}\n`);
      }
      const aliveReplies = await collectOsc(
        99,
        () => osc(99, "i=eggie-query:p=alive;"),
        (frames) => frames.some((frame) => frame.payload.includes("p=alive")),
        2_000,
      );
      const alive = aliveReplies.find((frame) => frame.payload.includes("p=alive"));
      if (!alive) throw new Error("OSC 99 alive query timed out");
      write(`  alive: ${alive.payload}\n`);
      osc(99, "i=eggie-test:p=close;");
    },
  },
  {
    id: "iterm-core",
    title: "iTerm2 查询、变量与剪贴板扩展",
    codes: "OSC 1337 ReportCellSize/SetUserVar/ReportVariable/Copy",
    interactive: true,
    run: async () => {
      const cell = await query(1337, "ReportCellSize");
      assertMatch(cell, /^ReportCellSize=\d+(?:\.\d+)?;\d+(?:\.\d+)?$/, "cell size");
      write(`  ${cell.payload}\n`);

      const expected = "feature/osc-test";
      osc(1337, `SetUserVar=branch=${b64(expected)}`);
      const variable = await query(1337, `ReportVariable=${b64("user.branch")}`);
      const actual = fromB64(variable.payload.replace(/^ReportVariable=/, ""));
      if (actual !== expected) throw new Error(`ReportVariable mismatch: ${actual}`);
      write(`  user.branch=${actual}\n`);

      if (await ask("允许用 OSC 1337 Copy 再次覆盖系统剪贴板吗？", false)) {
        osc(1337, `Copy=:${b64("Eggie OSC 1337 clipboard test")}`);
      }
    },
  },
  {
    id: "iterm-system",
    title: "iTerm2 URL、焦点和注意力请求",
    codes: "OSC 1337 OpenURL/StealFocus/RequestAttention",
    interactive: true,
    run: async () => {
      let ran = false;
      if (await ask("发送 OpenURL 测试吗？Eggie 应先显示授权提示。", false)) {
        ran = true;
        osc(1337, `OpenURL=:${b64("https://example.com/eggie-osc-test")}`);
        await visualPause("确认 Eggie 显示了打开链接授权提示");
      }
      if (await ask("测试 StealFocus 和 RequestAttention 吗？", true)) {
        ran = true;
        for (const mode of ["once", "yes", "fireworks", "no"]) {
          osc(1337, `RequestAttention=${mode}`);
          await sleep(400);
        }
        osc(1337, "StealFocus");
      }
      if (!ran) throw new SkipTest();
    },
  },
  {
    id: "iterm-images",
    title: "iTerm2 PNG/TIFF 单段与多段内联图片",
    codes: "OSC 1337 File/MultipartFile/FilePart/FileEnd",
    run: async () => {
      const name = b64("eggie-osc-test.png");
      const encoded = b64(TEST_PNG);
      write("  single-part inline PNG:\n");
      osc(1337, `File=name=${name};size=${TEST_PNG.length};inline=1;width=9;height=2:${encoded}`);
      write("\n\n");
      write("  multipart inline PNG:\n");
      const midpoint = Math.floor(encoded.length / 2);
      osc(
        1337,
        `MultipartFile=name=${name};size=${TEST_PNG.length};inline=1;width=9;height=2`,
      );
      osc(1337, `FilePart=${encoded.slice(0, midpoint)}`);
      osc(1337, `FilePart=${encoded.slice(midpoint)}`);
      osc(1337, "FileEnd");
      write("\n\n");
      write("  single-part inline TIFF:\n");
      osc(
        1337,
        `File=name=${b64("eggie-osc-test.tiff")};size=${TEST_TIFF.length};inline=1;width=9;height=2;preserveAspectRatio=0:${b64(TEST_TIFF)}`,
      );
      write("\n\n");
      write("  pixel box with default aspect ratio:\n");
      osc(
        1337,
        `File=name=${name};size=${TEST_PNG.length};inline=1;width=96px;height=72px:${encoded}`,
      );
      write("\n\n");
      write("  percentage width with automatic height:\n");
      osc(
        1337,
        `File=name=${name};size=${TEST_PNG.length};inline=1;width=25%;height=auto:${encoded}`,
      );
      write("\n\n");
      write("  intrinsic size (auto/auto):\n");
      osc(
        1337,
        `File=name=${name};size=${TEST_PNG.length};inline=1;width=auto;height=auto:${encoded}`,
      );
      write("\n\n");
      await visualPause(
        "应看到六张内联测试图，cell/px/%/auto 与等比或拉伸尺寸均正确且不应错位",
      );
    },
  },
  {
    id: "iterm-download",
    title: "iTerm2 单段与多段文件下载",
    codes: "OSC 1337 File/MultipartFile/FilePart/FileEnd (inline=0)",
    interactive: true,
    run: async () => {
      if (!(await ask("测试 iTerm2 文件下载吗？Eggie 会分别显示两个保存提示。", false))) {
        throw new SkipTest();
      }
      const first = Buffer.from("Eggie OSC 1337 single file\n");
      osc(1337, `File=name=${b64("eggie-osc-single.txt")};size=${first.length}:${b64(first)}`);
      await visualPause("处理第一个保存提示");

      const second = Buffer.from("Eggie OSC 1337 multipart file\n");
      const encoded = b64(second);
      osc(1337, `MultipartFile=name=${b64("eggie-osc-multipart.txt")};size=${second.length}`);
      osc(1337, `FilePart=${encoded.slice(0, Math.floor(encoded.length / 2))}`);
      osc(1337, `FilePart=${encoded.slice(Math.floor(encoded.length / 2))}`);
      osc(1337, "FileEnd");
      await visualPause("处理第二个保存提示");
    },
  },
  {
    id: "iterm-clear",
    title: "清除回滚缓冲区",
    codes: "OSC 1337 ClearScrollback",
    interactive: true,
    run: async () => {
      if (await ask("清空当前终端的历史回滚缓冲区吗？", false)) osc(1337, "ClearScrollback");
      else throw new SkipTest();
    },
  },
  {
    id: "rich-clipboard",
    title: "Kitty 富剪贴板写入、别名、列举和读取",
    codes: "OSC 5522 write/wdata/walias/read",
    interactive: true,
    run: async () => {
      if (!(await ask("允许 Kitty 富剪贴板测试覆盖系统剪贴板吗？", false))) {
        throw new SkipTest();
      }
      const id = `eggie-${Date.now()}`;
      const textMime = "text/plain;charset=utf-8";
      const htmlMime = "text/html";
      const writeReplies = await collectOsc(
        5522,
        () => {
          osc(5522, `type=write:id=${id}`);
          osc(5522, `type=wdata:id=${id}:mime=${b64(textMime)};${b64(TEST_CLIPBOARD_TEXT)}`);
          osc(5522, `type=wdata:id=${id}:mime=${b64(htmlMime)};${b64(`<b>${TEST_CLIPBOARD_TEXT}</b>`)}`);
          osc(5522, `type=walias:id=${id}:mime=${b64(textMime)};${b64("text/plain UTF8_STRING")}`);
          osc(5522, `type=wdata:id=${id};`);
        },
        (frames) => frames.some((frame) => frame.payload.includes("status=DONE")),
        3_000,
      );
      if (!writeReplies.some((frame) => frame.payload.includes("status=DONE"))) {
        throw new Error("OSC 5522 write did not finish");
      }

      const listReplies = await collectOsc(
        5522,
        () => osc(5522, `type=read:id=${id};${b64(".")}`),
        (frames) => frames.some((frame) => frame.payload.includes("status=DONE")),
        3_000,
      );
      write(`  list replies: ${listReplies.map((frame) => frame.payload).join(" | ")}\n`);

      const readReplies = await collectOsc(
        5522,
        () => osc(5522, `type=read:id=${id};${b64(textMime)}`),
        (frames) =>
          frames.some(
            (frame) => frame.payload.includes("status=DONE") || frame.payload.includes("status=EPERM"),
          ),
        3_000,
      );
      const status = readReplies.at(-1)?.payload ?? "timeout";
      write(`  read: ${status}\n`);
    },
  },
  {
    id: "paste-events",
    title: "Kitty 富剪贴板粘贴事件协商",
    codes: "DECSET 5522 + OSC 5522 paste announcement",
    interactive: true,
    run: async () => {
      if (!(await ask("测试粘贴事件吗？开启后请在 10 秒内执行一次粘贴。", false))) {
        throw new SkipTest();
      }
      write(`${CSI}?5522h`);
      const replies = await collectOsc(
        5522,
        () => write("  paste now…\n"),
        (frames) => frames.length > 0,
        10_000,
      );
      write(`${CSI}?5522l`);
      if (replies[0]) write(`  announcement: ${replies[0].payload}\n`);
      else write("  no OSC 5522 paste announcement received\n");
    },
  },
  {
    id: "kitty-transfer",
    title: "Kitty 文件传输（zlib + 分块）",
    codes: "OSC 5113 send/file/data/end_data/finish/cancel",
    interactive: true,
    run: async () => {
      if (!(await ask("测试 Kitty 文件传输吗？Eggie 会要求选择接收目录。", false))) {
        throw new SkipTest();
      }
      const transferId = `eggie-${Date.now()}`;
      const fileId = "file-1";
      const contents = randomBytes(12 * 1024);
      const compressed = deflateSync(contents);

      const ready = await collectOsc(
        5113,
        () => osc(5113, `ac=send;id=${transferId}`),
        (frames) => frames.some((frame) => frame.payload.includes(`id=${transferId}`)),
        120_000,
      );
      const readyStatus = decodedStatus(ready.at(-1));
      if (readyStatus !== "OK") {
        write(`  transfer not accepted: ${readyStatus ?? "timeout"}\n`);
        return;
      }

      const directory = await collectOsc(
        5113,
        () =>
          osc(
            5113,
            `ac=file;id=${transferId};fid=directory-1;ft=directory;n=${b64("eggie-osc-5113")}`,
          ),
        (frames) => frames.some((frame) => decodedStatus(frame) === "OK"),
        5_000,
      );
      if (!directory.some((frame) => decodedStatus(frame) === "OK")) {
        throw new Error(`Kitty directory start failed: ${decodedStatus(directory.at(-1)) ?? "timeout"}`);
      }

      const started = await collectOsc(
        5113,
        () =>
          osc(
            5113,
            `ac=file;id=${transferId};fid=${fileId};ft=regular;n=${b64("eggie-osc-5113/payload.bin")};sz=${contents.length};zip=zlib;prm=420`,
          ),
        (frames) => frames.some((frame) => decodedStatus(frame) === "STARTED"),
        5_000,
      );
      const startStatus = decodedStatus(started.at(-1));
      if (startStatus !== "STARTED") {
        throw new Error(`Kitty file start failed: ${startStatus ?? "timeout"}`);
      }

      for (let offset = 0; offset < compressed.length; offset += 4_096) {
        const chunk = compressed.subarray(offset, offset + 4_096);
        const progress = await collectOsc(
          5113,
          () => osc(5113, `ac=data;id=${transferId};fid=${fileId};d=${b64(chunk)}`),
          (frames) => frames.some((frame) => decodedStatus(frame) === "PROGRESS"),
          5_000,
        );
        if (!progress.some((frame) => decodedStatus(frame) === "PROGRESS")) {
          throw new Error(`Kitty data chunk failed: ${decodedStatus(progress.at(-1)) ?? "timeout"}`);
        }
      }
      const finishedFile = await collectOsc(
        5113,
        () => osc(5113, `ac=end_data;id=${transferId};fid=${fileId}`),
        (frames) => frames.some((frame) => decodedStatus(frame) === "OK"),
        10_000,
      );
      const fileStatus = decodedStatus(finishedFile.at(-1));
      if (fileStatus !== "OK") {
        throw new Error(`Kitty file data failed: ${fileStatus ?? "timeout"}`);
      }

      const finished = await collectOsc(
        5113,
        () => osc(5113, `ac=finish;id=${transferId}`),
        (frames) => frames.some((frame) => decodedStatus(frame) === "OK"),
        10_000,
      );
      const finalStatus = decodedStatus(finished.at(-1));
      if (finalStatus !== "OK") {
        throw new Error(`Kitty transfer finish failed: ${finalStatus ?? "timeout"}`);
      }
      write(`  saved eggie-osc-5113/payload.bin (${contents.length} bytes)\n`);

      const canceled = await collectOsc(
        5113,
        () => osc(5113, `ac=cancel;id=eggie-cancel-${Date.now()}`),
        (frames) => frames.some((frame) => decodedStatus(frame) === "CANCELED"),
        3_000,
      );
      if (!canceled.some((frame) => decodedStatus(frame) === "CANCELED")) {
        throw new Error(`Kitty cancel failed: ${decodedStatus(canceled.at(-1)) ?? "timeout"}`);
      }
    },
  },
];

function restoreTerminal(): void {
  osc(9, "4;0");
  osc(22, "default");
  osc(50, "CursorShape=0");
  osc(104, "");
  osc(110, "");
  osc(111, "");
  osc(112, "");
  write(`${CSI}?5522l${CSI}0m`);
}

function usage(): void {
  write(`Eggie OSC integration test (Bun)\n\n`);
  write(`Usage:\n`);
  write(`  bun scripts/osc-test.ts [--list] [--only=id,id] [--yes] [--dry-run]\n\n`);
  write(`Options:\n`);
  write(`  --list          list all tests without emitting escape sequences\n`);
  write(`  --only=id,id    run selected test IDs\n`);
  write(`  --yes           accept every interactive/destructive prompt\n`);
  write(`  --dry-run       show the selected execution plan only\n\n`);
}

async function main(): Promise<void> {
  if (argv.includes("--help") || argv.includes("-h")) {
    usage();
    return;
  }

  const unknown = selected ? [...selected].filter((id) => !tests.some((test) => test.id === id)) : [];
  if (unknown.length > 0) throw new Error(`unknown test ID(s): ${unknown.join(", ")}`);
  const chosen = selected ? tests.filter((test) => selected.has(test.id)) : tests;

  if (listOnly || dryRun) {
    usage();
    for (const test of chosen) {
      write(`${test.id.padEnd(18)} ${test.codes.padEnd(58)} ${test.title}${test.interactive ? " [interactive]" : ""}\n`);
    }
    return;
  }

  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    throw new Error("run interactively inside Eggie, or use --list/--dry-run");
  }

  write(`\nEggie OSC integration test — ${chosen.length} groups\n`);
  write("Ctrl-C stops the run; terminal colors/modes are restored on exit.\n");
  let passed = 0;
  let failed = 0;
  let skipped = 0;

  const interrupt = () => {
    restoreTerminal();
    write("\nInterrupted.\n");
    process.exit(130);
  };
  process.once("SIGINT", interrupt);

  try {
    for (const test of chosen) {
      write(`\n\x1b[1;36m[${test.id}]\x1b[0m ${test.title} — ${test.codes}\n`);
      try {
        await test.run();
        passed++;
        write("\x1b[32m  PASS\x1b[0m\n");
      } catch (error) {
        if (error instanceof SkipTest) {
          skipped++;
          write(`\x1b[33m  SKIP\x1b[0m${error.message ? ` ${error.message}` : ""}\n`);
          continue;
        }
        failed++;
        const message = error instanceof Error ? error.message : String(error);
        write(`\x1b[31m  FAIL\x1b[0m ${message}\n`);
      }
    }
  } finally {
    process.off("SIGINT", interrupt);
    restoreTerminal();
    osc(2, "Eggie OSC test complete");
  }

  write(`\nDone: ${passed} passed, ${skipped} skipped, ${failed} failed.\n`);
  if (failed > 0) process.exitCode = 1;
}

await main();
