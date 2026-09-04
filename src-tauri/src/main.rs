#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    state: String,
    ts: u64,
    last_turn_line: String,
}

// 读取 ~/.claude/claude-pet/*.json，返回原始会话列表；聚合与 FSM 在 TS 端完成。
#[tauri::command]
fn read_sessions() -> Vec<Session> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join(".claude/claude-pet");
    let mut out = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    let state = v["state"].as_str().unwrap_or("").to_string();
                    let transcript = v["transcript"].as_str().unwrap_or("");
                    // 仅工作态需要读 transcript 尾部（Esc 中断检测），空闲态省掉这次 IO
                    let last_turn_line = if state == "thinking" || state == "tool" {
                        last_turn_line(transcript)
                    } else {
                        String::new()
                    };
                    out.push(Session {
                        state,
                        ts: v["ts"].as_u64().unwrap_or(0),
                        last_turn_line,
                    });
                }
            }
        }
    }
    out
}

// transcript 尾部最后一条 user/assistant 行，用于识别 Esc 中断（"interrupted by user"）
fn last_turn_line(path: &str) -> String {
    let Ok(mut f) = fs::File::open(path) else {
        return String::new();
    };
    let Ok(size) = f.metadata().map(|m| m.len()) else {
        return String::new();
    };
    const CHUNK: u64 = 8192;
    if f.seek(SeekFrom::Start(size.saturating_sub(CHUNK))).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf)
        .lines()
        .filter(|l| l.contains("\"type\":\"user\"") || l.contains("\"type\":\"assistant\""))
        .next_back()
        .unwrap_or("")
        .to_string()
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_sessions, quit])
        .run(tauri::generate_context!())
        .expect("error while running ClaudePet");
}
