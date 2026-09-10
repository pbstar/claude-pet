// Anthropic Messages ⇄ OpenAI Chat Completions 协议转换（请求侧 + 非流式响应）
// 覆盖 Code tab 实际用到的子集：未知请求字段丢弃、未知响应块跳过，不崩。
// 流式响应翻译见 stream.rs。
use serde_json::{json, Value};

// ─────────────────────────── 请求：Anthropic → OpenAI ───────────────────────────

pub fn convert_request(body: &Value, model: &str) -> Value {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut out = json!({"model": model, "stream": stream});

    // o 系列（o1/o3/o4-mini…）只认 max_completion_tokens，发 max_tokens 会被拒
    if let Some(mt) = body.get("max_tokens") {
        if is_o_series(model) {
            out["max_completion_tokens"] = mt.clone();
        } else {
            out["max_tokens"] = mt.clone();
        }
    }
    for (src, dst) in [("temperature", "temperature"), ("top_p", "top_p")] {
        if let Some(v) = body.get(src) {
            if !v.is_null() {
                out[dst] = v.clone();
            }
        }
    }
    if let Some(v) = body.get("stop_sequences").and_then(Value::as_array) {
        if !v.is_empty() {
            out["stop"] = Value::Array(v.clone());
        }
    }
    // thinking → reasoning_effort：只对支持该参数的模型注入，否则上游报未知字段
    if supports_reasoning_effort(model) {
        if let Some(effort) = resolve_reasoning_effort(body) {
            out["reasoning_effort"] = json!(effort);
        }
    }
    // 流式必须显式声明 include_usage，否则上游 SSE 不回 usage，token 统计全为 0
    if stream {
        out["stream_options"] = json!({"include_usage": true});
    }

    // 顶层 system（string 或 block 数组）→ 一条 system 消息置于最前
    let mut msgs: Vec<Value> = Vec::new();
    if let Some(sys) = system_text(body.get("system")) {
        msgs.push(json!({"role": "system", "content": sys}));
    }
    if let Some(arr) = body.get("messages").and_then(Value::as_array) {
        for m in arr {
            convert_message(m, &mut msgs);
        }
    }
    out["messages"] = Value::Array(msgs);

    // tools：input_schema → parameters（根 schema 补 object 类型，否则严格上游拒收）
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?;
                Some(json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.get("description").cloned().unwrap_or(json!("")),
                        "parameters": clean_schema(t.get("input_schema").cloned().unwrap_or(json!({"type":"object"}))),
                    }
                }))
            })
            .collect();
        if !converted.is_empty() {
            out["tools"] = Value::Array(converted);
        }
    }
    if let Some(tc) = body.get("tool_choice") {
        out["tool_choice"] = match tc.get("type").and_then(Value::as_str) {
            Some("any") => json!({"type": "required"}),
            Some("tool") => json!({"type": "function", "function": {"name": tc["name"].clone()}}),
            Some("auto") => json!("auto"),
            _ => Value::Null,
        };
        if out["tool_choice"].is_null() {
            out.as_object_mut().unwrap().remove("tool_choice");
        }
    }
    out
}

fn system_text(system: Option<&Value>) -> Option<String> {
    let texts: Vec<String> = match system? {
        Value::String(s) => vec![s.clone()],
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        _ => return None,
    };
    // 客户端会在 system 开头塞一行 x-anthropic-billing-header，其 cch= 每次请求都变，
    // 不剥离会让上游前缀缓存永远不命中（cc-switch #2350 同款处理）
    let joined: Vec<&str> = texts.iter().map(|t| strip_billing_header(t)).filter(|t| !t.is_empty()).collect();
    if joined.is_empty() {
        None
    } else {
        Some(joined.join("\n"))
    }
}

fn strip_billing_header(text: &str) -> &str {
    const PREFIX: &str = "x-anthropic-billing-header:";
    if !text.starts_with(PREFIX) {
        return text;
    }
    match text.find(['\n', '\r']) {
        Some(end) => text[end..].trim_start_matches(['\n', '\r']),
        None => "",
    }
}

// 根 schema 缺 type 时补 object（OpenAI 要求），并去掉上游普遍不认的 format:"uri"
fn clean_schema(schema: Value) -> Value {
    clean_schema_inner(schema, true)
}

fn clean_schema_inner(mut schema: Value, is_root: bool) -> Value {
    let Some(obj) = schema.as_object_mut() else {
        return schema;
    };
    if is_root && !obj.contains_key("type") {
        obj.insert("type".to_string(), json!("object"));
        obj.entry("properties").or_insert_with(|| json!({}));
    }
    if obj.get("format").and_then(Value::as_str) == Some("uri") {
        obj.remove("format");
    }
    if let Some(props) = obj.get_mut("properties").and_then(Value::as_object_mut) {
        for (_, v) in props.iter_mut() {
            *v = clean_schema_inner(v.clone(), false);
        }
    }
    if let Some(items) = obj.get_mut("items") {
        *items = clean_schema_inner(items.clone(), false);
    }
    schema
}

// o 系列推理模型：o1 / o3 / o4-mini …
fn is_o_series(model: &str) -> bool {
    let m = model.to_lowercase();
    m.len() > 1 && m.starts_with('o') && m.as_bytes()[1].is_ascii_digit()
}

// 支持 reasoning_effort 的模型：o 系列 + gpt-5 及以后
fn supports_reasoning_effort(model: &str) -> bool {
    let m = model.to_lowercase();
    is_o_series(&m)
        || m.strip_prefix("gpt-")
            .and_then(|r| r.chars().next())
            .is_some_and(|c| c.is_ascii_digit() && c >= '5')
}

// Anthropic thinking 预算 → OpenAI reasoning_effort。
// 优先取显式 output_config.effort；否则按 budget_tokens 分档，未知值不注入
fn resolve_reasoning_effort(body: &Value) -> Option<&'static str> {
    if let Some(effort) = body.pointer("/output_config/effort").and_then(Value::as_str) {
        return match effort {
            "low" => Some("low"),
            "medium" => Some("medium"),
            "high" => Some("high"),
            "max" => Some("xhigh"),
            _ => None,
        };
    }
    let thinking = body.get("thinking")?;
    match thinking.get("type").and_then(Value::as_str) {
        Some("adaptive") => Some("xhigh"),
        Some("enabled") => match thinking.get("budget_tokens").and_then(Value::as_u64) {
            Some(b) if b < 4_000 => Some("low"),
            Some(b) if b < 16_000 => Some("medium"),
            _ => Some("high"),
        },
        _ => None,
    }
}

// 逐条转换消息；OpenAI 不允许相邻同角色 → 连续同角色合并
fn convert_message(m: &Value, out: &mut Vec<Value>) {
    let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
    let blocks = m.get("content");
    let blocks_arr: Vec<Value> = match blocks {
        Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };

    // tool_result 块 → 独立的 role:"tool" 消息（可多条），其余并入 user 消息
    if role == "user" {
        let mut texts: Vec<Value> = Vec::new();
        let mut images: Vec<Value> = Vec::new();
        for b in &blocks_arr {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_result") => {
                    flush_user_msg(&mut texts, &mut images, out);
                    let content = match b.get("content") {
                        Some(Value::String(s)) => json!(s.as_str()),
                        Some(Value::Array(inner)) => json!(inner.iter()
                            .filter_map(|c| c.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>().join("\n")),
                        _ => json!(""),
                    };
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(json!("")),
                        "content": content,
                    }));
                }
                Some("image") => {
                    if let Some(img) = convert_image(b) {
                        images.push(img);
                    }
                }
                Some("text") | None => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        texts.push(json!({"type": "text", "text": t}));
                    }
                }
                _ => {} // thinking 等未知块丢弃
            }
        }
        flush_user_msg(&mut texts, &mut images, out);
        return;
    }

    // assistant：text + tool_use → content 数组（function 块）
    let mut content: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for b in &blocks_arr {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    if !t.is_empty() {
                        content.push(json!({"type": "text", "text": t}));
                    }
                }
            }
            Some("tool_use") => {
                tool_calls.push(json!({
                    "id": b.get("id").cloned().unwrap_or(json!("")),
                    "type": "function",
                    "function": {
                        "name": b.get("name").cloned().unwrap_or(json!("")),
                        "arguments": b.get("input").map(|i| i.to_string()).unwrap_or_default(),
                    }
                }));
            }
            _ => {} // thinking 丢弃
        }
    }
    if content.is_empty() && tool_calls.is_empty() {
        content.push(json!({"type": "text", "text": ""}));
    }
    let mut msg = json!({"role": "assistant", "content": content});
    if !tool_calls.is_empty() {
        msg["tool_calls"] = Value::Array(tool_calls);
    }
    merge_or_push(out, msg);
}

fn convert_image(b: &Value) -> Option<Value> {
    let source = b.get("source")?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let media = source.get("media_type").and_then(Value::as_str)?;
    let data = source.get("data").and_then(Value::as_str)?;
    Some(json!({"type": "image_url", "image_url": {"url": format!("data:{media};base64,{data}")}}))
}

// 同一条 user 消息：纯文本 → string；带图 → content 数组
fn flush_user_msg(texts: &mut Vec<Value>, images: &mut Vec<Value>, out: &mut Vec<Value>) {
    if texts.is_empty() && images.is_empty() {
        return;
    }
    let msg = if images.is_empty() && texts.len() == 1 {
        json!({"role": "user", "content": texts[0]["text"].clone()})
    } else {
        let mut content = texts.clone();
        content.extend(images.iter().cloned());
        json!({"role": "user", "content": content})
    };
    texts.clear();
    images.clear();
    // 相邻同 role 合并（Anthropic 允许连续 user，OpenAI 不允许）
    merge_or_push(out, msg);
}

fn merge_or_push(out: &mut Vec<Value>, msg: Value) {
    let role = msg["role"].as_str().unwrap_or("");
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) && !role.is_empty() {
            // 相邻同角色合并：content 统一成数组拼接，tool_calls 保留双方
            let mut content = match last.get("content") {
                Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
                Some(Value::Array(a)) => a.clone(),
                _ => vec![],
            };
            match msg.get("content") {
                Some(Value::String(s)) => content.push(json!({"type": "text", "text": s})),
                Some(Value::Array(a)) => content.extend(a.clone()),
                _ => {}
            }
            if !content.is_empty() {
                last["content"] = Value::Array(content);
            }
            if let Some(tc) = msg.get("tool_calls") {
                let arr = last.as_object_mut().unwrap().entry("tool_calls").or_insert_with(|| json!([]));
                arr.as_array_mut().unwrap().extend(tc.as_array().cloned().unwrap_or_default());
            }
            return;
        }
    }
    out.push(msg);
}

// ─────────────────────────── 响应：OpenAI → Anthropic（非流式） ───────────────────────────

pub fn convert_response(resp: &Value, src_model: &str) -> Value {
    let mut content: Vec<Value> = Vec::new();
    if let Some(msg) = resp.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message")) {
        // DeepSeek/MiMo 等把思考放在 message.reasoning_content
        if let Some(t) = msg.get("reasoning_content").and_then(Value::as_str) {
            if !t.is_empty() {
                content.push(json!({"type": "thinking", "thinking": t}));
            }
        }
        if let Some(t) = msg.get("content").and_then(Value::as_str) {
            if !t.is_empty() {
                // 压缩摘要请求会带 <analysis>/<summary> 标签：丢弃 analysis 段、
                // 剥掉 summary 标签——原样返回会让标签出现在可见输出里
                let text = crate::stream::sanitize_internal_tags(t);
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
            }
        }
        if let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                content.push(json!({
                    "type": "tool_use",
                    "id": call.get("id").cloned().unwrap_or(json!("")),
                    "name": call["function"]["name"].clone(),
                    "input": serde_json::from_str::<Value>(
                        call["function"]["arguments"].as_str().unwrap_or("{}")
                    ).unwrap_or(json!({})),
                }));
            }
        }
    }
    if content.is_empty() {
        content.push(json!({"type": "text", "text": ""}));
    }

    let stop = resp["choices"][0]["finish_reason"].as_str().unwrap_or("stop");
    json!({
        "id": resp.get("id").cloned().unwrap_or(json!("msg_unknown")),
        "type": "message",
        "role": "assistant",
        "model": src_model,
        "content": content,
        "stop_reason": map_stop_reason(stop),
        "stop_sequence": Value::Null,
        "usage": map_usage(resp.get("usage")),
    })
}

// OpenAI usage → Anthropic usage。prompt_tokens 含缓存命中，Anthropic 的
// input_tokens 不含——三桶互斥（input + cache_read + cache_creation == prompt），
// 不减会让缓存被重复计入 input
pub(crate) fn map_usage(u: Option<&Value>) -> Value {
    let Some(u) = u else {
        return json!({"input_tokens": 0, "output_tokens": 0});
    };
    let prompt = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = cache_read_tokens(u);
    let creation = cache_write_tokens(u);
    let mut v = json!({
        "input_tokens": prompt.saturating_sub(cached).saturating_sub(creation),
        "output_tokens": output,
    });
    if cached > 0 {
        v["cache_read_input_tokens"] = json!(cached);
    }
    if creation > 0 {
        v["cache_creation_input_tokens"] = json!(creation);
    }
    v
}

// 缓存命中：直传字段优先（部分兼容上游直接给 Anthropic 形态），否则 OpenAI 嵌套 details
pub(crate) fn cache_read_tokens(u: &Value) -> u64 {
    u.get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64))
        .unwrap_or(0)
}

pub(crate) fn cache_write_tokens(u: &Value) -> u64 {
    u.get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            u.pointer("/prompt_tokens_details/cache_write_tokens")
                .or_else(|| u.pointer("/input_tokens_details/cache_write_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0)
}

pub(crate) fn map_stop_reason(r: &str) -> &'static str {
    match r {
        "tool_calls" | "function_call" => "tool_use",
        "length" => "max_tokens",
        _ => "end_turn",
    }
}
