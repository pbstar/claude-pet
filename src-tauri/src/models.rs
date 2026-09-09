// models.json 读写（activeId 即「切换」语义）+ Claude Code CLI settings.json 校正
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const PROXY_ADDR: &str = "127.0.0.1:15721";
pub const PROXY_URL: &str = "http://127.0.0.1:15721";
pub const DESKTOP_ROUTE: &str = "/claude-desktop";

// 上游协议格式：anthropic = Anthropic Messages 端点（换头直通）；
// openai = Chat Completions 端点（协议转换）
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpstreamFormat {
    Anthropic,
    Openai,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub format: UpstreamFormat,
    #[serde(default)]
    pub base_url: String,
    pub token: String,
    // 目标模型名：发来的 claude-* 角色模型名固定替换为它
    pub model: String,
    pub supports_1m: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsState {
    pub active_id: Option<String>,
    desktop_token: String,
    models: Vec<ModelEntry>,
}

impl ModelsState {
    pub fn load() -> ModelsState {
        Self::load_from(&models_path())
    }

    fn load_from(path: &Path) -> ModelsState {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&models_path())
    }

    // tmp + rename 原子写，权限 600
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn active(&self) -> Option<&ModelEntry> {
        let id = self.active_id.as_deref()?;
        self.models.iter().find(|m| m.id == id)
    }

    pub fn models(&self) -> &[ModelEntry] {
        &self.models
    }

    pub fn desktop_token(&self) -> &str {
        &self.desktop_token
    }

    // desktopToken 不存在则生成
    pub fn ensure_token(&mut self) {
        if self.desktop_token.is_empty() {
            self.desktop_token = format!("pet-{}", uuid::Uuid::new_v4().simple());
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert(
        &mut self,
        id: Option<String>,
        name: String,
        format: UpstreamFormat,
        base_url: String,
        token: String,
        model: String,
        supports_1m: bool,
    ) -> Option<String> {
        match id {
            Some(id) => {
                let m = self.models.iter_mut().find(|m| m.id == id)?;
                m.name = name;
                m.format = format;
                m.base_url = base_url;
                m.token = token;
                m.model = model;
                m.supports_1m = supports_1m;
                Some(id)
            }
            None => {
                let entry = ModelEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    name,
                    format,
                    base_url,
                    token,
                    model,
                    supports_1m,
                };
                // 首个条目自动成为 active
                if self.active_id.is_none() {
                    self.active_id = Some(entry.id.clone());
                }
                let id = entry.id.clone();
                self.models.push(entry);
                Some(id)
            }
        }
    }

    // 删除 active 条目时，activeId 移交给相邻条目，避免悬空
    pub fn remove(&mut self, id: &str) {
        let was_active = self.active_id.as_deref() == Some(id);
        let pos = self.models.iter().position(|m| m.id == id);
        self.models.retain(|m| m.id != id);
        if was_active {
            self.active_id = pos
                .and_then(|i| self.models.get(i).or_else(|| self.models.last()))
                .map(|m| m.id.clone());
        }
    }

    pub fn set_active(&mut self, id: &str) -> bool {
        if self.models.iter().any(|m| m.id == id) {
            self.active_id = Some(id.to_string());
            true
        } else {
            false
        }
    }
}

fn models_path() -> PathBuf {
    dirs_home().join(".claude/claude-pet/models.json")
}

pub fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

// 每次启动校正 settings.json：env 指向本地代理（接管 CLI 的 /v1/messages 通路），
// 并清理把请求钉死在旧模型上的别名键（模型替换统一在代理侧做）
pub fn ensure_code_settings() {
    let settings_path = dirs_home().join(".claude/settings.json");
    if !settings_path.exists() {
        return;
    }
    let mut settings = match fs::read_to_string(&settings_path) {
        Ok(s) => serde_json::from_str::<serde_json::Value>(&s).unwrap_or(serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };

    let env = match settings.as_object_mut() {
        Some(obj) => obj.entry("env").or_insert_with(|| serde_json::json!({})),
        None => return,
    };
    let changed = {
        let env = env.as_object_mut().unwrap();
        let mut changed = false;
        for (k, v) in [
            ("ANTHROPIC_BASE_URL", PROXY_URL),
            ("ANTHROPIC_AUTH_TOKEN", "PROXY_MANAGED"),
        ] {
            if env.get(k).and_then(|v| v.as_str()) != Some(v) {
                env.insert(k.to_string(), serde_json::json!(v));
                changed = true;
            }
        }
        // 这些键会把请求钉死在旧模型上；模型替换统一在代理侧
        for k in [
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
            "ANTHROPIC_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            if env.remove(k).is_some() {
                changed = true;
            }
        }
        changed
    };
    if changed {
        let _ = fs::write(&settings_path, serde_json::to_string_pretty(&settings).unwrap());
    }
}
