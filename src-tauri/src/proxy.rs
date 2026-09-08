// 本地代理（127.0.0.1:15721）：服务 Claude Desktop 的 Code tab
// 入口 /claude-desktop/*（token 校验）；Code tab 内嵌的 CLI 由 Desktop 注入
// host-creds（ANTHROPIC_BASE_URL 指向该前缀），不经 CLI 原生的 /v1/messages
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

use crate::convert;
use crate::models::{ModelEntry, ModelsState, UpstreamFormat, DESKTOP_ROUTE, PROXY_ADDR};
use crate::stream::{self, StreamTranslator};

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

// 共享 reqwest 客户端：无总超时，连接超时 30s
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
    // Desktop 网关前缀剥掉，上游只认原生路径
    let uri = strip_desktop_prefix(&parts.uri);
    let headers = parts.headers;
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return err_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    forward(&headers, &uri, &bytes).await
}

fn strip_desktop_prefix(uri: &Uri) -> Uri {
    let pq = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let stripped = pq.strip_prefix(DESKTOP_ROUTE).unwrap_or(pq);
    let stripped = if stripped.starts_with('/') { stripped.to_string() } else { format!("/{stripped}") };
    Uri::builder().path_and_query(stripped).build().unwrap_or_else(|_| Uri::from_static("/"))
}

// ─────────────────────────── 转发主流程 ───────────────────────────

async fn forward(headers: &HeaderMap, uri: &Uri, body_bytes: &[u8]) -> Response {
    // 每请求现读 models.json 取 active：换条目对下一个请求即时生效，跑着的流不断
    let state = ModelsState::load();
    let Some(entry) = state.active().cloned() else {
        return err_response(StatusCode::SERVICE_UNAVAILABLE, "claude-pet: no active model entry");
    };

    let body: Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(e) => return err_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {e}")),
    };
    let src_model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    let target_model = resolve_model(&src_model, &entry);

    match entry.format {
        UpstreamFormat::Anthropic => forward_anthropic(&entry, headers, uri, &body, &target_model).await,
        UpstreamFormat::Openai => forward_openai(&entry, &body, &target_model, &src_model).await,
    }
}

// 模型替换与 1M：claude-* 角色模型名固定替换为条目目标模型；
// 主体不含角色关键词时原样透传（兜底，如用户显式 /model 指定上游原生模型名）
fn resolve_model(src_model: &str, entry: &ModelEntry) -> String {
    let main = src_model
        .strip_suffix("[1m]")
        .or_else(|| src_model.strip_suffix("[1M]"))
        .unwrap_or(src_model);
    // "custom" = Desktop 选择器写死的 claude-custom 哨兵名（desktop_profile.rs），同样替换
    let is_role = ["fable", "opus", "sonnet", "haiku", "custom"]
        .iter()
        .any(|r| main.to_lowercase().contains(r));
    // [1m] 是客户端本地能力标记，上游普遍拒收——只用来驱动 context-1m beta 头，
    // 绝不拼回模型名
    if is_role || main.is_empty() {
        entry.model.clone()
    } else {
        main.to_string()
    }
}

// ── 管道 A：anthropic 换头直通 ──
async fn forward_anthropic(
    entry: &ModelEntry,
    headers: &HeaderMap,
    uri: &Uri,
    body: &Value,
    target_model: &str,
) -> Response {
    let mut out_body = body.clone();
    out_body["model"] = json!(target_model);

    // 上游 URL = baseUrl + 原样 path/query（与客户端直连上游拼接等价）
    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let url = format!("{}/{}", entry.base_url.trim_end_matches('/'), path.trim_start_matches('/'));
    let url = url.trim_end_matches('/').to_string();

    let mut req = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", entry.token));
    // 剥入来的 host/authorization/x-api-key（已重新注入），其余头透传；
    // anthropic-beta 头按条目 supports1m 收敛 context-1m
    for (k, v) in headers.iter() {
        let key = k.as_str();
        if matches!(key, "host" | "authorization" | "x-api-key" | "content-length" | "accept-encoding") {
            continue;
        }
        if key == "anthropic-beta" {
            if let Some(filtered) = filter_beta(v, entry.supports_1m) {
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
fn filter_beta(v: &HeaderValue, supports_1m: bool) -> Option<String> {
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

// ── 管道 B：openai 协议转换 ──
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
    // 非流式；或 stream:true 但上游回 JSON → 整读按非流式转换
    let v: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return err_response(StatusCode::BAD_GATEWAY, format!("upstream decode failed: {e}")),
    };
    if !wants_stream {
        return Json(convert::convert_response(&v, src_model)).into_response();
    }
    // 单个 SSE 序列回给客户端
    let events = stream::full_message_to_sse(&convert::convert_response(&v, src_model));
    sse_response(vec![Ok(stream::render_sse(&events))])
}

// 流式核心路径：上游 SSE → 翻译状态机 → 客户端 SSE，逐块直通不缓冲
fn sse_translated(resp: reqwest::Response, src_model: &str) -> Response {
    let init = StreamState {
        translator: StreamTranslator::new(src_model.to_string()),
        buffer: String::new(),
        utf8_rem: Vec::new(),
        upstream: Box::pin(resp.bytes_stream().map(|r| r.map_err(std::io::Error::other))),
        finished: false,
        done: false,
    };
    let stream = futures_util::stream::unfold(init, |mut st| async move {
        loop {
            // 收尾事件已发过 → 流到此结束，否则会重复发 message_stop
            if st.done {
                return None;
            }
            // 1. 消化缓冲区里的完整 SSE 帧
            let mut events: Vec<Value> = Vec::new();
            let (frames, consumed) = stream::parse_sse_frames(&st.buffer);
            st.buffer.drain(..consumed);
            let mut saw_done = false;
            for f in &frames {
                if f == stream::DONE_SENTINEL {
                    saw_done = true;
                    continue;
                }
                if let Ok(chunk) = serde_json::from_str::<Value>(f) {
                    events.extend(st.translator.feed(&chunk));
                }
            }
            // 2. 上游结束（[DONE] 或断流）→ 收尾
            if saw_done || st.finished {
                events.extend(st.translator.finish());
                st.done = true;
                return Some((Ok::<String, std::io::Error>(stream::render_sse(&events)), st));
            }
            if !events.is_empty() {
                return Some((Ok(stream::render_sse(&events)), st));
            }
            // 3. 缓冲区无完整帧，拉下一块上游字节
            match st.upstream.next().await {
                Some(Ok(bytes)) => stream::append_utf8_safe(&mut st.buffer, &mut st.utf8_rem, &bytes),
                Some(Err(_)) | None => st.finished = true,
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
    buffer: String,
    // 跨块截断的多字节 UTF-8 残片，等下一块补齐再解析
    utf8_rem: Vec<u8>,
    upstream: std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send>>,
    // 上游已结束，待发收尾事件
    finished: bool,
    // 收尾事件已发，流终止
    done: bool,
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

// 上游错误体翻译为 Anthropic 错误结构，保留状态码（客户端重试逻辑依赖）
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
