#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::PathBuf;

use serde::Serialize;

#[derive(Serialize)]
struct Session {
    state: String,
    ts: u64,
    transcript: String,
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
                    out.push(Session {
                        state: v["state"].as_str().unwrap_or("").to_string(),
                        ts: v["ts"].as_u64().unwrap_or(0),
                        transcript: v["transcript"].as_str().unwrap_or("").to_string(),
                    });
                }
            }
        }
    }
    out
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_sessions])
        .run(tauri::generate_context!())
        .expect("error while running ClaudePet");
}
