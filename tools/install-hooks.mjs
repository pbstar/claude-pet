#!/usr/bin/env node
// 安装 claude-pet 的 hooks 到 ~/.claude/settings.json（幂等：先剥离旧的再追加；首次安装自动备份）
// usage: node tools/install-hooks.mjs
import { readFileSync, writeFileSync, existsSync, copyFileSync, mkdirSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";

const HOME = homedir();
const PET_DIR = join(HOME, ".claude", "claude-pet");
const HOOK_DEST = join(PET_DIR, "hook.sh");
const SETTINGS = join(HOME, ".claude", "settings.json");
const MARKER = PET_DIR; // 用路径作识别标记，卸载时据此剥离

const here = join(fileURLToPath(import.meta.url), "..");
const hookSrc = join(here, "hook.sh");

// 1. 复制并授权 hook.sh
mkdirSync(PET_DIR, { recursive: true });
copyFileSync(hookSrc, HOOK_DEST);
chmodSync(HOOK_DEST, 0o755);

// 2. 备份 settings.json（仅首次）
let settings = {};
if (existsSync(SETTINGS)) {
  settings = JSON.parse(readFileSync(SETTINGS, "utf8"));
  const bak = SETTINGS + ".bak-claude-pet";
  if (!existsSync(bak)) copyFileSync(SETTINGS, bak);
}
settings.hooks = settings.hooks || {};

const isOurs = (command) => command.includes(MARKER);
const cmd = (evt) => `bash '${HOOK_DEST}' ${evt}`;

const stripOurs = (arr) =>
  (arr || [])
    .map((e) => ({ ...e, hooks: (e.hooks || []).filter((h) => !isOurs(h.command || "")) }))
    .filter((e) => (e.hooks || []).length > 0);

const addUnmatched = (evt, command) => {
  settings.hooks[evt] = stripOurs(settings.hooks[evt]);
  settings.hooks[evt].push({ hooks: [{ type: "command", command }] });
};
const addMatched = (evt, command) => {
  settings.hooks[evt] = stripOurs(settings.hooks[evt]);
  settings.hooks[evt].push({ matcher: "*", hooks: [{ type: "command", command }] });
};

// 事件 → 状态映射
addUnmatched("UserPromptSubmit", cmd("thinking"));
addMatched("PreToolUse", cmd("tool"));
addMatched("PostToolUse", cmd("thinking"));
addUnmatched("Notification", cmd("notify"));
addMatched("PermissionRequest", cmd("permission"));
addUnmatched("Stop", cmd("done"));
addUnmatched("SessionEnd", cmd("clean"));

writeFileSync(SETTINGS, JSON.stringify(settings, null, 2) + "\n");
console.log("claude-pet hooks 已安装 →", SETTINGS);
console.log("hook 脚本 →", HOOK_DEST);
console.log("备份（首次）→", SETTINGS + ".bak-claude-pet");
