#!/usr/bin/env node
// 一次性脚本：从原仓库 Sources/CrabFrames.swift 提取 20 帧 base64 PNG，导出到 public/crab/
// usage: node tools/export-sprite.mjs <CrabFrames.swift 路径> [输出目录]
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const src = process.argv[2];
const outDir = process.argv[3] || join(process.cwd(), "public", "crab");
if (!src) {
  console.error("用法: node tools/export-sprite.mjs <CrabFrames.swift> [输出目录]");
  process.exit(1);
}

const text = readFileSync(src, "utf8");
const m = text.match(/let clawdCrabFramePNGs:\s*\[String\]\s*=\s*\[([\s\S]*?)\]/);
if (!m) {
  console.error("未在文件中找到 clawdCrabFramePNGs 数组");
  process.exit(1);
}
const b64s = [...m[1].matchAll(/"([A-Za-z0-9+/=]+)"/g)].map((x) => x[1]);
if (b64s.length === 0) {
  console.error("未提取到任何帧数据");
  process.exit(1);
}
mkdirSync(outDir, { recursive: true });
b64s.forEach((b64, i) => {
  writeFileSync(join(outDir, `${i}.png`), Buffer.from(b64, "base64"));
});
console.log(`已导出 ${b64s.length} 帧 → ${outDir}`);
