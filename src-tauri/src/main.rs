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
    // transcript jsonl 的 mtime（秒）；0 = 读不到。CLI 在沙箱外实时写它，
    // 是 hook 冻结/丢失时的活跃度兜底信号（TS 端 state.ts 使用）
    transcript_mtime: u64,
}

// 读取 ~/.claude/claude-pet/*.json，返回原始会话列表；聚合与 FSM 在 TS 端完成。
// 每次调用顺带节流校验 settings.json hooks（外部程序可能整键丢弃，见 maybe_verify_hooks）
#[tauri::command]
fn read_sessions() -> Vec<Session> {
    maybe_verify_hooks();
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join(".claude/claude-pet");
    let mut out = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            // 只认 hook 写的 <uuid>.json；跳过 hook.sh 等同目录其他文件
            if path.extension().and_then(|e| e.to_str()) != Some("json")
                || path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| !is_uuid(s))
                    .unwrap_or(true)
            {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    let state = v["state"].as_str().unwrap_or("").to_string();
                    let ts = v["ts"].as_u64().unwrap_or(0);
                    let transcript = v["transcript"].as_str().unwrap_or("");
                    // 先取 mtime 再决定是否读尾部：TS 端只在 mtime > ts（hook 信号已过期）时才用
                    // lastTurnLine，其余情况读 8KB 纯属浪费（陈旧会话的 transcript 可达数 MB）
                    let transcript_mtime = if transcript.is_empty() {
                        0
                    } else {
                        std::fs::metadata(transcript)
                            .and_then(|m| m.modified())
                            .map(|t| {
                                t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
                            })
                            .unwrap_or(0)
                    };
                    let last_turn_line =
                        if transcript_mtime > ts { last_turn_line(transcript) } else { String::new() };
                    out.push(Session { state, ts, last_turn_line, transcript_mtime });
                }
            }
        }
    }
    out
}

// 8-4-4-4-12 十六进制段，够过滤 hook 产物即可，不追求严格 RFC 4122
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
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
    ensure_hooks_installed();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_sessions, quit])
        .run(tauri::generate_context!())
        .expect("error while running ClaudePet");
}

fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

// ── hooks 自愈 ──
// 外部程序（实测 Claude Desktop 改设置时）会按自己认识的配置子集重写 settings.json，
// 整键丢弃不认识的 hooks。read_sessions 每 5s 节流校验一次：发现 pet hooks 不在就重装
// （ensure_hooks_installed 幂等）。正在运行的会话可能不热加载，新会话立即恢复。
static HOOKS_CHECK_TS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn maybe_verify_hooks() {
    use std::sync::atomic::Ordering;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = HOOKS_CHECK_TS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 5 {
        return;
    }
    HOOKS_CHECK_TS.store(now, Ordering::Relaxed);
    if pet_hooks_present() {
        return;
    }
    eprintln!("claude-pet: hooks missing from settings.json, reinstalling");
    ensure_hooks_installed();
}

// settings.json 的 hooks 里任一 command 引用 pet 目录即视为在位
fn pet_hooks_present() -> bool {
    let path = dirs_home().join(".claude/settings.json");
    let Ok(s) = fs::read_to_string(&path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
        return false;
    };
    let Some(events) = v["hooks"].as_object() else {
        return false;
    };
    events.values().filter_map(|a| a.as_array()).flatten().any(|entry| {
        entry["hooks"].as_array().is_some_and(|hs| {
            hs.iter()
                .any(|h| h["command"].as_str().is_some_and(|c| c.contains(".claude/claude-pet")))
        })
    })
}

// 首次启动自安装：写 hook.sh 到 ~/.claude/claude-pet/，并把 7 个事件 hook 合并进 settings.json
fn ensure_hooks_installed() {
    let home = dirs_home();
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
            entry["hooks"].as_array().is_none_or(|hs| {
                hs.iter()
                    .all(|h| h["command"].as_str().is_none_or(|c| !c.contains(".claude/claude-pet")))
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
