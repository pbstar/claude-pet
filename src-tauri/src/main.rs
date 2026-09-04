#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde::Serialize;

// 内嵌的 hook 脚本（编译期从 src-tauri/hook.sh 读入）
const HOOK_SCRIPT: &str = include_str!("../hook.sh");

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

// 首次启动自安装：写 hook.sh 到 ~/.claude/claude-pet/，并把 7 个事件 hook 合并进 settings.json
fn ensure_hooks_installed() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let pet_dir = home.join(".claude/claude-pet");
    let hook_dest = pet_dir.join("hook.sh");
    let settings_path = home.join(".claude/settings.json");

    // 1. 写 hook.sh（幂等；每次启动刷新，保证升级后脚本同步）
    if fs::create_dir_all(&pet_dir).is_err() {
        return;
    }
    if fs::write(&hook_dest, HOOK_SCRIPT).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&hook_dest, fs::Permissions::from_mode(0o755));
    }

    // 2. 合并 hooks 到 settings.json（先剥离旧的再追加，幂等）
    let mut settings = match fs::read_to_string(&settings_path) {
        Ok(s) => serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    if settings_path.exists() {
        let bak = settings_path.with_extension("json.bak-claude-pet");
        if !bak.exists() {
            let _ = fs::copy(&settings_path, &bak);
        }
    }

    let hook_cmd = format!("bash '{}'", hook_dest.display());
    const EVENTS: &[(&str, &str, bool)] = &[
        // (事件名, hook 参数, 是否带 matcher "*")
        ("UserPromptSubmit", "thinking", false),
        ("PreToolUse", "tool", true),
        ("PostToolUse", "thinking", true),
        ("Notification", "notify", false),
        ("PermissionRequest", "permission", true),
        ("Stop", "done", false),
        ("SessionEnd", "clean", false),
    ];

    if settings.get("hooks").and_then(|h| h.as_object()).is_none() {
        settings["hooks"] = serde_json::json!({});
    }
    let hooks = settings["hooks"].as_object_mut().unwrap();

    for &(event, arg, matched) in EVENTS {
        let arr = hooks
            .entry(event)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .unwrap();

        // 剥离本应用已存在的 hook 条目（按 command 是否引用 pet 目录判断）
        arr.retain(|entry| {
            entry["hooks"].as_array().map_or(true, |hs| {
                hs.iter().all(|h| {
                    h["command"]
                        .as_str()
                        .map_or(true, |c| !c.contains(".claude/claude-pet"))
                })
            })
        });

        let mut new_entry = serde_json::json!({});
        if matched {
            new_entry["matcher"] = serde_json::json!("*");
        }
        new_entry["hooks"] = serde_json::json!([{
            "type": "command",
            "command": format!("{hook_cmd} {arg}")
        }]);
        arr.push(new_entry);
    }

    if let Ok(s) = serde_json::to_string_pretty(&settings) {
        let _ = fs::write(&settings_path, s);
    }
}

fn main() {
    ensure_hooks_installed();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_sessions, quit])
        .run(tauri::generate_context!())
        .expect("error while running ClaudePet");
}
