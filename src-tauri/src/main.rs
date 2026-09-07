#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde::Serialize;
use tauri::Manager;

mod convert;
mod desktop_profile;
mod models;
mod proxy;

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
            // 只认 hook 写的 <uuid>.json；跳过 models.json、hook.sh 等同目录其他文件
            if path.extension().and_then(|e| e.to_str()) != Some("json")
                || path.file_stem().and_then(|s| s.to_str()).map(|s| uuid::Uuid::parse_str(s).is_err()).unwrap_or(true)
            {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    let state = v["state"].as_str().unwrap_or("").to_string();
                    let transcript = v["transcript"].as_str().unwrap_or("");
                    // permission 态不需要 transcript 信号；其余状态读尾部+mtime
                    // （working 态用于 Esc 中断检测，非 permission 态用于 TS 端活跃度兜底/复活判断）
                    let (last_turn_line, transcript_mtime) = if state != "permission" && !transcript.is_empty() {
                        (
                            last_turn_line(transcript),
                            std::fs::metadata(transcript)
                                .and_then(|m| m.modified())
                                .map(|t| {
                                    t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
                                })
                                .unwrap_or(0),
                        )
                    } else {
                        (String::new(), 0)
                    };
                    out.push(Session {
                        state,
                        ts: v["ts"].as_u64().unwrap_or(0),
                        last_turn_line,
                        transcript_mtime,
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

// ── 模型管理 invoke 命令（供 manager.html / 菜单使用）──

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelEntryDto {
    id: String,
    name: String,
    format: models::UpstreamFormat,
    base_url: String,
    token: String,
    model: String,
    supports_1m: bool,
    active: bool,
}

fn to_dto(state: &models::ModelsState) -> Vec<ModelEntryDto> {
    let active_id = state.active_id.clone();
    state
        .models()
        .iter()
        .map(|m| ModelEntryDto {
            id: m.id.clone(),
            name: m.name.clone(),
            format: m.format,
            base_url: m.base_url.clone(),
            token: m.token.clone(),
            model: m.model.clone(),
            supports_1m: m.supports_1m,
            active: active_id.as_deref() == Some(&m.id),
        })
        .collect()
}

#[tauri::command]
fn list_models() -> Vec<ModelEntryDto> {
    let state = models::ModelsState::load();
    to_dto(&state)
}

#[tauri::command]
#[allow(non_snake_case)]
fn save_model(
    id: Option<String>,
    name: String,
    format: models::UpstreamFormat,
    baseUrl: String,
    token: String,
    model: String,
    supports1m: bool,
) -> Result<(), String> {
    let name = name.trim().to_string();
    let base_url = baseUrl.trim().to_string();
    let model = model.trim().to_string();
    if name.is_empty() || base_url.is_empty() || model.is_empty() || token.is_empty() {
        return Err("名称、Base URL、Token、模型名均为必填".into());
    }
    let mut state = models::ModelsState::load();
    let new_id = state
        .upsert(id, name, format, base_url, token, model, supports1m)
        .ok_or_else(|| "条目不存在".to_string())?;
    state.save().map_err(|e| e.to_string())?;
    let _ = new_id;
    Ok(())
}

#[tauri::command]
fn delete_model(id: String) -> Result<(), String> {
    let mut state = models::ModelsState::load();
    state.remove(&id);
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// 右键菜单点条目 = 只改 activeId，一个原子写（第五节）
#[tauri::command]
fn switch_model(id: String) -> Result<bool, String> {
    let mut state = models::ModelsState::load();
    let ok = state.set_active(&id);
    if ok {
        state.save().map_err(|e| e.to_string())?;
    }
    Ok(ok)
}

#[tauri::command]
fn proxy_status() -> bool {
    proxy::is_running()
}

// 菜单右键触发重试绑定（端口空出来后，无需重启 pet 即可恢复）
// 绑定成功时补做一次配置校正——首启绑定失败时配置没写，这里兜底
#[tauri::command]
async fn retry_proxy() -> bool {
    if proxy::is_running() {
        return true;
    }
    if proxy::spawn().await {
        ensure_models_file();
        models::ensure_code_settings();
        desktop_profile::ensure_desktop_profile();
        true
    } else {
        false
    }
}

#[tauri::command]
fn open_manager(app: tauri::AppHandle) {
    open_manager_window(&app);
}

// 管理小窗：按需创建第二个 WebView 小窗，关即销毁
// always_on_top 必开：本应用是 LSUIElement 后台应用，无法成为活动应用，
// 普通窗口即使 set_focus 也压在前台软件的窗口后面（表现为"弹窗没出现"）
fn open_manager_window(app: &tauri::AppHandle) {
    use tauri::WebviewUrl;
    if let Some(win) = app.get_webview_window("manager") {
        let _ = win.set_focus();
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "manager",
        WebviewUrl::App("manager.html".into()),
    )
    .title("模型管理")
    .inner_size(420.0, 560.0)
    .resizable(false)
    .always_on_top(true)
    .focused(true)
    .build();
}

// ── 启动装配（顺序见文档六：先绑端口，成功才写配置）──

fn main() {
    ensure_hooks_installed();

    // tokio runtime：代理服务 + 启动期安装流程
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let proxy_up = rt.block_on(proxy::spawn());

    if proxy_up {
        // 顺序照文档六：models.json（含 token 生成）→ settings.json → Desktop profile
        // （Desktop profile 依赖 token，必须在 models.json 之后）
        ensure_models_file();
        models::ensure_code_settings();
        desktop_profile::ensure_desktop_profile();
    } else {
        eprintln!("claude-pet: proxy not started, skip config sync (settings untouched)");
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            read_sessions,
            quit,
            list_models,
            save_model,
            delete_model,
            switch_model,
            proxy_status,
            retry_proxy,
            open_manager
        ])
        .run(tauri::generate_context!())
        .expect("error while running ClaudePet");
}

// models.json：不存在则建空列表；desktopToken 不存在则生成（六.2）
fn ensure_models_file() {
    let dir = dirs_home().join(".claude/claude-pet");
    let _ = fs::create_dir_all(&dir);
    let mut state = models::ModelsState::load();
    if state.desktop_token().is_empty() {
        state.ensure_token();
    }
    let _ = state.save();
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
        entry["hooks"]
            .as_array()
            .map(|hs| {
                hs.iter().any(|h| {
                    h["command"].as_str().map_or(false, |c| c.contains(".claude/claude-pet"))
                })
            })
            .unwrap_or(false)
    })
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
