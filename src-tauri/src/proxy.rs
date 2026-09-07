// 本地代理（127.0.0.1:15721）：Claude Code 与 Claude Desktop 共用
// 两个入口同一流程：/v1/messages ← Code；/claude-desktop/* ← Desktop（token 校验）
// 按 active 条目 format 分流：anthropic 换头直通；openai 协议转换（convert.rs）
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::convert::{self, StreamTranslator};
use crate::models::{ModelEntry, ModelsState, UpstreamFormat, DESKTOP_ROUTE, PROXY_ADDR};

pub struct ProxyState {
    running: AtomicBool,
}

#[derive(Clone)]
pub struct AppStore(Arc<ProxyState>);

static STORE: OnceLock<AppStore> = OnceLock::new();

pub fn store() -> AppStore {
    STORE
        .get_or_init(|| AppStore(Arc::new(ProxyState { running: AtomicBool::new(false) })))
        .clone()
}

pub fn is_running() -> bool {
    store().0.running.load(Ordering::Relaxed)
}

// 共享 reqwest 客户端：无总超时，连接超时 30s（4.4）
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client")
    })
}

// 启动代理；绑定失败（端口被占，如 cc-switch 未退）返回 false，不写任何配置
pub async fn spawn() -> bool {
    let app = Router::new()
        .route("/v1/messages", any(handle_messages))
        .route("/v1/messages/count_tokens", any(handle_count_tokens))
        .route(DESKTOP_ROUTE, any(handle_desktop))
        .route(&format!("{DESKTOP_ROUTE}/{{*path}}"), any(handle_desktop))
        .with_state(store())
        // axum 默认 2MB 请求体上限会拒掉带截图 base64 的请求，放开到 MAX_BODY
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY));

    let listener = match tokio::net::TcpListener::bind(PROXY_ADDR).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("claude-pet proxy: bind {PROXY_ADDR} failed: {e}");
            return false;
        }
    };
    store().0.running.store(true, Ordering::Relaxed);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
        store().0.running.store(false, Ordering::Relaxed);
    });
    true
}

// ─────────────────────────── 入口路由 ───────────────────────────

const MAX_BODY: usize = 200 * 1024 * 1024;

// Claude Code：POST /v1/messages（token 是占位符 PROXY_MANAGED，不校验）
async fn handle_messages(_state: State<AppStore>, req: Request) -> Response {
    read_body_and_forward(req).await
}

// count_tokens：openai 上游无对应端点，本地粗估（≈字符数/4，够 CLI 展示用）；
// anthropic 上游本可直通，但该接口 CLI 仅作显示，统一走估算即可
async fn handle_count_tokens(req: Request) -> Response {
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), MAX_BODY).await else {
        return err_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return err_response(StatusCode::BAD_REQUEST, "invalid JSON");
    };
    // 粗估：system + 全部消息文本长度 / 4
    let mut chars = v["system"].as_str().map(str::len).unwrap_or(0);
    if let Some(arr) = v["messages"].as_array() {
        for m in arr {
            match &m["content"] {
                Value::String(s) => chars += s.len(),
                Value::Array(blocks) => {
                    for b in blocks {
                        chars += b["text"].as_str().map(str::len).unwrap_or(0);
                        chars += b["content"].as_str().map(str::len).unwrap_or(0);
                    }
                }
                _ => {}
            }
        }
    }
    (StatusCode::OK, Json(json!({"input_tokens": (chars / 4).max(1)}))).into_response()
}

// Claude Desktop：/claude-desktop 前缀，校验 Authorization: Bearer <desktopToken>
async fn handle_desktop(_state: State<AppStore>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let token = ModelsState::load().desktop_token().to_string();
    let ok = !token.is_empty()
        && parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| t == token);
    if !ok {
        return err_response(StatusCode::UNAUTHORIZED, "invalid desktop token");
    }
    // Desktop 网关前缀剥掉，上游只认原生路径（/v1/messages 等）
    let uri = strip_desktop_prefix(&parts.uri);
    let headers = parts.headers;
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return err_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    forward(&headers, &uri, &bytes).await
}

async fn read_body_and_forward(req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return err_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    forward(&parts.headers, &parts.uri, &bytes).await
}

fn strip_desktop_prefix(uri: &Uri) -> Uri {
    let pq = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let stripped = pq.strip_prefix(DESKTOP_ROUTE).unwrap_or(pq);
    let stripped = if stripped.starts_with('/') { stripped.to_string() } else { format!("/{stripped}") };
    Uri::builder().path_and_query(stripped).build().unwrap_or_else(|_| Uri::from_static("/"))
}

// ─────────────────────────── 转发主流程 ───────────────────────────

async fn forward(headers: &HeaderMap, uri: &Uri, body_bytes: &[u8]) -> Response {
    // 每请求现读 models.json 取 active：换条目对下一个请求即时生效，跑着的流不断（4.4）
    let state = ModelsState::load();
    let Some(entry) = state.active().cloned() else {
        return err_response(StatusCode::SERVICE_UNAVAILABLE, "claude-pet: no active model entry");
    };

    let body: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(e) => return err_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {e}")),
    };
    let src_model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    let (target_model, had_1m) = resolve_model(&src_model, &entry);

    match entry.format {
        UpstreamFormat::Anthropic => forward_anthropic(&entry, headers, uri, &body, &target_model, had_1m).await,
        UpstreamFormat::Openai => forward_openai(&entry, &body, &target_model, &src_model).await,
    }
}

// 4.3 模型替换与 1M：claude-* 角色模型名固定替换为条目目标模型；
// 主体不含角色关键词时原样透传（兜底，如用户显式 /model 指定上游原生模型名）
fn resolve_model(src_model: &str, entry: &ModelEntry) -> (String, bool) {
    let (main, had_1m) = match src_model.strip_suffix("[1m]").or_else(|| src_model.strip_suffix("[1M]")) {
        Some(main) => (main, true),
        None => (src_model, false),
    };
    // "custom" = Desktop 选择器写死的 claude-custom 哨兵名（desktop_profile.rs），同样替换
    let is_role = ["fable", "opus", "sonnet", "haiku", "custom"]
        .iter()
        .any(|r| main.to_lowercase().contains(r));
    let target = if is_role || main.is_empty() {
        entry.model.clone()
    } else {
        main.to_string()
    };
    // supports1m=true 保留后缀（重建为小写 [1m]）；false 剥掉
    let final_model = if had_1m && entry.supports_1m {
        format!("{target}[1m]")
    } else {
        target
    };
    (final_model, had_1m && entry.supports_1m)
}

// ── 管道 A：anthropic 换头直通（4.0）──
async fn forward_anthropic(
    entry: &ModelEntry,
    headers: &HeaderMap,
    uri: &Uri,
    body: &Value,
    target_model: &str,
    had_1m: bool,
) -> Response {
    let mut out_body = body.clone();
    out_body["model"] = json!(target_model);

    // 上游 URL = baseUrl + 原样 path/query（与 CLI 直连上游拼接等价）
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let url = format!("{}/{}", entry.base_url.trim_end_matches('/'), path.trim_start_matches('/'));
    let url = url.trim_end_matches('/').to_string();

    let mut req = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", entry.token));
    // 剥入来的 host/authorization/x-api-key（已重新注入），其余头透传；
    // anthropic-beta 头按条目 supports1m 收敛 context-1m（4.3）
    for (k, v) in headers.iter() {
        let key = k.as_str();
        if matches!(key, "host" | "authorization" | "x-api-key" | "content-length" | "accept-encoding") {
            continue;
        }
        if key == "anthropic-beta" {
            if let Some(filtered) = filter_beta(v, had_1m, entry.supports_1m) {
                req = req.header(key, filtered);
            }
            continue;
        }
        if let Ok(val) = v.to_str() {
            req = req.header(key, val);
        }
    }
    req = req.body(serde_json::to_vec(&out_body).unwrap_or_default());

    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            // 响应体（含 SSE）字节级直通，不解析
            let stream = resp.bytes_stream().map(|r| r.map_err(|e| std::io::Error::other(e)));
            let mut builder = Response::builder().status(status);
            if let Ok(v) = HeaderValue::from_str(&ct) {
                builder = builder.header("content-type", v);
            }
            builder.body(axum::body::Body::from_stream(stream)).unwrap()
        }
        Err(e) => err_response(StatusCode::BAD_GATEWAY, format!("upstream connect failed: {e}")),
    }
}

// 放行规则：条目 supports1m=true → 保留全部 beta（含 context-1m）；
// 否则移除 context-1m-*，其余放行（请求本身带不带 [1m] 不影响其他 beta 特性）
fn filter_beta(v: &HeaderValue, _had_1m: bool, supports_1m: bool) -> Option<String> {
    let beta = v.to_str().ok()?;
    if supports_1m {
        return Some(beta.to_string());
    }
    let filtered: Vec<&str> = beta
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty() && !f.contains("context-1m"))
        .collect();
    if filtered.is_empty() { None } else { Some(filtered.join(", ")) }
}

// ── 管道 B：openai 协议转换（4.1/4.2）──
async fn forward_openai(entry: &ModelEntry, body: &Value, target_model: &str, src_model: &str) -> Response {
    let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let out_body = convert::convert_request(body, target_model);
    let url = format!("{}/chat/completions", entry.base_url.trim_end_matches('/'));

    let req = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", entry.token))
        .json(&out_body);

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return err_response(StatusCode::BAD_GATEWAY, format!("upstream connect failed: {e}")),
    };
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| {
                if text.is_empty() { format!("upstream {status}") } else { text }
            });
        return upstream_error_response(status.as_u16(), message);
    }

    let is_sse = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));

    if wants_stream && is_sse {
        return sse_translated(resp, src_model);
    }
    // 非流式；或 stream:true 但上游回 JSON → 整读按非流式转换（4.4）
    let v: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return err_response(StatusCode::BAD_GATEWAY, format!("upstream decode failed: {e}")),
    };
    if !wants_stream {
        return Json(convert::convert_response(&v, src_model)).into_response();
    }
    // 单个 SSE 序列回给客户端
    let events = full_message_to_sse(&convert::convert_response(&v, src_model));
    sse_response(vec![Ok(render_sse(&events))])
}

// 流式核心路径：上游 SSE → 翻译状态机 → 客户端 SSE，逐块直通不缓冲
fn sse_translated(resp: reqwest::Response, src_model: &str) -> Response {
    let translator = StreamTranslator::new(src_model.to_string());
    // 流开头：message_start 先行（4.2：连接建立即发，content 随块逐个出现）
    let pending: Vec<Result<String, std::io::Error>> = vec![Ok(render_sse(&translator.start_events()))];
    let init = StreamState {
        translator,
        pending,
        buffer: String::new(),
        stop_reason: "end_turn".to_string(),
        upstream: Box::pin(resp.bytes_stream().map(|r| r.map_err(std::io::Error::other))),
        finished: false,
    };
    let stream = futures_util::stream::unfold(init, |mut st| async move {
        loop {
            // 1. 待发队列优先（message_start）
            if !st.pending.is_empty() {
                let frame = st.pending.remove(0);
                return Some((frame, st));
            }
            // 2. 消化缓冲区里的完整 SSE 帧
            let mut events: Vec<Value> = Vec::new();
            let (frames, consumed) = convert::parse_sse_frames(&st.buffer);
            st.buffer.drain(..consumed);
            let mut saw_done = false;
            for f in &frames {
                if f == convert::DONE_SENTINEL {
                    saw_done = true;
                    continue;
                }
                let Ok(chunk) = serde_json::from_str::<Value>(f) else { continue };
                if let Some(fr) = chunk["choices"][0]["finish_reason"].as_str() {
                    if !fr.is_empty() {
                        st.stop_reason = fr.to_string();
                    }
                }
                events.extend(st.translator.feed(&chunk));
            }
            if saw_done {
                events.extend(st.translator.finish(&st.stop_reason));
                st.finished = true;
            }
            if !events.is_empty() {
                return Some((Ok::<String, std::io::Error>(render_sse(&events)), st));
            }
            if st.finished {
                return None;
            }
            // 3. 缓冲区无完整帧，拉下一块上游字节
            match st.upstream.next().await {
                Some(Ok(bytes)) => match String::from_utf8(bytes.to_vec()) {
                    Ok(s) => st.buffer.push_str(&s),
                    Err(_) => {
                        let evs = st.translator.finish(&st.stop_reason);
                        st.finished = true;
                        return Some((Ok(render_sse(&evs)), st));
                    }
                },
                // 上游断流/结束：按当前 stop_reason 收尾（含空流兜底）
                Some(Err(_)) | None => {
                    let evs = st.translator.finish(&st.stop_reason);
                    st.finished = true;
                    return Some((Ok(render_sse(&evs)), st));
                }
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}

struct StreamState {
    translator: StreamTranslator,
    pending: Vec<Result<String, std::io::Error>>,
    buffer: String,
    stop_reason: String,
    upstream: std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send>>,
    finished: bool,
}

// Anthropic SSE 事件序列 → 响应字节（event: 类型 + data: JSON + 空行）
fn render_sse(events: &[Value]) -> String {
    let mut out = String::new();
    for ev in events {
        out.push_str(&format!("event: {}\ndata: {}\n\n", ev["type"].as_str().unwrap_or("message"), ev));
    }
    out
}

fn sse_response(frames: Vec<Result<String, std::io::Error>>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from_stream(futures_util::stream::iter(frames)))
        .unwrap()
}

// ─────────────────────────── 连通性测试 ───────────────────────────

// 管理弹窗「测试」按钮：用表单当前值（不落盘、不影响 active）按正式转发同款
// URL 拼接与鉴权头打一发最小对话请求。2xx 即连通；尽力提取应答文本片段，
// 证明上游真的能出内容（只测 TCP/域名会漏掉 token 错、模型名错这类问题）
pub async fn check_connectivity(
    format: UpstreamFormat,
    base_url: String,
    token: String,
    model: String,
) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/');
    let token = token.trim();
    let model = model.trim();
    if base.is_empty() || token.is_empty() || model.is_empty() {
        return Err("Base URL、Token、模型名需先填写".into());
    }

    // 独立 client：带总超时，避免测试按钮挂在无超时的共享 client 上长时间无反馈
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    let ping = json!({"role": "user", "content": "ping"});
    let (url, body) = match format {
        // baseUrl 为 ANTHROPIC_BASE_URL 形态，追加 /v1/messages（同 forward_anthropic）
        UpstreamFormat::Anthropic => (
            format!("{base}/v1/messages"),
            json!({"model": model, "max_tokens": 8, "messages": [ping]}),
        ),
        // baseUrl 为 Chat Completions 根地址（同 forward_openai）
        UpstreamFormat::Openai => (
            format!("{base}/chat/completions"),
            json!({"model": model, "max_tokens": 8, "messages": [ping], "stream": false}),
        ),
    };

    let mut req = client
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));
    if matches!(format, UpstreamFormat::Anthropic) {
        req = req.header("anthropic-version", "2023-06-01");
    }

    let started = std::time::Instant::now();
    let resp = match req.json(&body).send().await {
        Ok(r) => r,
        Err(e) => return Err(format!("请求失败：{e}")),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let ms = started.elapsed().as_millis();

    if !status.is_success() {
        return Err(format!("上游返回 {status}（{ms}ms）：{}", truncate(text.trim(), 160)));
    }
    // 回包尽力解出应答片段；解析不出（如推理模型吞掉 max_tokens）只报状态
    let reply = serde_json::from_str::<Value>(&text).ok().and_then(|v| {
        let s = match format {
            UpstreamFormat::Anthropic => v["content"][0]["text"].as_str(),
            UpstreamFormat::Openai => v["choices"][0]["message"]["content"].as_str(),
        };
        s.map(str::to_string).filter(|s| !s.trim().is_empty())
    });
    Ok(match reply {
        Some(s) => format!("连通成功（{ms}ms），应答「{}」", truncate(s.trim(), 24)),
        None => format!("连通成功（HTTP {status}，{ms}ms）"),
    })
}

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

// 上游错误体翻译为 Anthropic 错误结构，保留状态码（CLI 重试逻辑依赖，4.4）
fn upstream_error_response(status: u16, message: String) -> Response {
    let typ = match status {
        429 => "rate_limit_error",
        401 | 403 => "authentication_error",
        404 => "not_found_error",
        400 => "invalid_request_error",
        _ => "api_error",
    };
    let sc = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = json!({"type": "error", "error": {"type": typ, "message": message}});
    (sc, Json(body)).into_response()
}

fn err_response(status: StatusCode, message: impl std::fmt::Display) -> Response {
    upstream_error_response(status.as_u16(), message.to_string())
}

// 完整 Anthropic message JSON → 单轮 SSE 事件序列（上游非 SSE 兜底）
fn full_message_to_sse(msg: &Value) -> Vec<Value> {
    let usage = &msg["usage"];
    let mut events = vec![json!({
        "type": "message_start",
        "message": {
            "id": msg["id"].clone(),
            "type": "message",
            "role": "assistant",
            "model": msg["model"].clone(),
            "content": [],
            "stop_reason": Value::Null,
            "usage": {"input_tokens": usage["input_tokens"].as_u64().unwrap_or(0), "output_tokens": 0},
        }
    })];
    for (i, block) in msg["content"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        let is_tool = block["type"].as_str() == Some("tool_use");
        let start_block = if is_tool {
            json!({"type": "tool_use", "id": block["id"].clone(), "name": block["name"].clone(), "input": {}})
        } else {
            json!({"type": "text", "text": ""})
        };
        events.push(json!({"type": "content_block_start", "index": i, "content_block": start_block}));
        let delta = if is_tool {
            json!({"type": "input_json_delta", "partial_json": block["input"].to_string()})
        } else {
            json!({"type": "text_delta", "text": block["text"].as_str().unwrap_or("")})
        };
        events.push(json!({"type": "content_block_delta", "index": i, "delta": delta}));
        events.push(json!({"type": "content_block_stop", "index": i}));
    }
    events.push(json!({
        "type": "message_delta",
        "delta": {"stop_reason": msg["stop_reason"].clone(), "stop_sequence": Value::Null},
        "usage": {"output_tokens": usage["output_tokens"].as_u64().unwrap_or(0)},
    }));
    events.push(json!({"type": "message_stop"}));
    events
}
