// Claude Desktop 3p 网关 profile 写入
// 仅当 Claude-3p 目录存在才做（未装 Desktop 静默跳过）；写前全量快照、任一步失败回滚，
// 幂等风格与 ensure_hooks_installed 一致。
use std::fs;
use std::path::PathBuf;

use crate::models::{ModelsState, DESKTOP_ROUTE, PROXY_URL};

// 固定 profile id（pet 专用槽位，重装复写同一条目）
const PROFILE_ID: &str = "00000000-0000-4000-8000-000000157210";
const PROFILE_NAME: &str = "ClaudePet";

// 单一哨兵条目：Desktop 选择器只出现 "Claude Custom" 一项且无 1M 派生（supports1m=false）。
// 切换无意义——代理 resolve_model 会把任何角色/custom 模型名统一替换为 active 条目目标，
// 实际线路只由 pet 右键菜单决定；名字含 "custom" 由代理侧特判替换（proxy.rs）
const DESKTOP_MODELS: &[(&str, bool)] = &[("claude-custom", false)];

pub fn ensure_desktop_profile() {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let dir3p = home.join("Library/Application Support/Claude-3p");
    if !dir3p.is_dir() {
        return; // 未装 Desktop（3p 形态），静默跳过
    }
    let lib = dir3p.join("configLibrary");

    // 涉及的全部文件：先快照（None = 原本不存在），再统一写入，任一步失败整体回滚
    let config_normal = home.join("Library/Application Support/Claude/claude_desktop_config.json");
    let config_3p = dir3p.join("claude_desktop_config.json");
    let profile = lib.join(format!("{PROFILE_ID}.json"));
    let meta = lib.join("_meta.json");
    let targets = [config_normal.clone(), config_3p.clone(), profile.clone(), meta.clone()];

    let snapshots: Vec<(PathBuf, Option<Vec<u8>>)> =
        targets.iter().map(|p| (p.clone(), fs::read(p).ok())).collect();

    let token = ModelsState::load().desktop_token().to_string();
    let gateway_base = format!("{PROXY_URL}{DESKTOP_ROUTE}");
    let models: Vec<serde_json::Value> = DESKTOP_MODELS
        .iter()
        .map(|(name, s1m)| serde_json::json!({"name": name, "supports1m": s1m}))
        .collect();

    let ok = set_deployment_mode_3p(&config_normal, &config_3p).is_ok()
        && write_profile(&profile, &lib, &token, &gateway_base, &models).is_ok()
        && write_meta(&meta, &lib).is_ok();

    if !ok {
        for (path, data) in &snapshots {
            match data {
                Some(bytes) => {
                    let _ = fs::write(path, bytes);
                }
                None => {
                    let _ = fs::remove_file(path);
                }
            }
        }
        eprintln!("claude-pet: desktop profile write failed, rolled back");
    }
}

// 两份 claude_desktop_config.json 顶层 deploymentMode 改为 "3p"，其余键不动
fn set_deployment_mode_3p(config_normal: &PathBuf, config_3p: &PathBuf) -> std::io::Result<()> {
    for p in [config_normal, config_3p] {
        if !p.exists() {
            continue;
        }
        let mut v: serde_json::Value = fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}));
        if v.get("deploymentMode").and_then(|m| m.as_str()) != Some("3p") {
            v["deploymentMode"] = serde_json::json!("3p");
            fs::write(p, serde_json::to_string_pretty(&v).unwrap())?;
        }
    }
    Ok(())
}

fn write_profile(
    profile: &PathBuf,
    lib: &PathBuf,
    token: &str,
    gateway_base: &str,
    models: &[serde_json::Value],
) -> std::io::Result<()> {
    fs::create_dir_all(lib)?;
    // labelOverride 不做；选择器显示 "Claude Custom"（claude-custom 由代理特判替换为 active 目标）
    // inferenceGatewayAuthScheme 显式声明 bearer：代理侧只认 Authorization: Bearer
    fs::write(
        profile,
        serde_json::to_string_pretty(&serde_json::json!({
            "inferenceProvider": "gateway",
            "inferenceGatewayBaseUrl": gateway_base,
            "inferenceGatewayApiKey": token,
            "inferenceGatewayAuthScheme": "bearer",
            "inferenceModels": models,
            "disableDeploymentModeChooser": true,
        }))
        .unwrap(),
    )
}

// _meta.json 登记 pet profile 并设 appliedId；用户自建的其他 profile 条目保留
fn write_meta(meta: &PathBuf, lib: &PathBuf) -> std::io::Result<()> {
    fs::create_dir_all(lib)?;
    let existing: serde_json::Value = fs::read_to_string(meta)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}));
    let mut list = existing["entries"].as_array().cloned().unwrap_or_default();
    list.retain(|e| e["id"].as_str() != Some(PROFILE_ID));
    list.push(serde_json::json!({"id": PROFILE_ID, "name": PROFILE_NAME}));
    fs::write(
        meta,
        serde_json::to_string_pretty(&serde_json::json!({
            "appliedId": PROFILE_ID,
            "entries": list,
        }))
        .unwrap(),
    )
}
