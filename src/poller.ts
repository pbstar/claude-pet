// 轮询数据源：调用 Rust 端 read_sessions 命令，读取 ~/.claude/claude-pet/*.json
import { invoke } from "@tauri-apps/api/core";
import type { RawSession } from "./state";

export function fetchSessions(): Promise<RawSession[]> {
  return invoke<RawSession[]>("read_sessions");
}
