// Anthropic Messages ⇄ OpenAI Chat Completions 协议转换（管道 B，4.1/4.2 节）
// 覆盖 CLI/Desktop 实际用到的子集：未知请求字段丢弃、未知响应块跳过，不崩。
use serde_json::{json, Value};

// ─────────────────────────── 请求：Anthropic → OpenAI ───────────────────────────

pub fn convert_request(body: &Value, model: &str) -> Value {
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut out = json!({
        "model": model,
        "stream": stream,
    });

    if let Some(mt) = body.get("max_tokens").and_then(Value::as_u64) {
        out["max_tokens"] = json!(mt);
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

    // tools：input_schema → parameters
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
                        "parameters": t.get("input_schema").cloned().unwrap_or(json!({"type":"object"})),
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
    match system? {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            if texts.is_empty() { None } else { Some(texts.join("\n")) }
        }
        _ => None,
    }
}

// 逐条转换消息；OpenAI 不允许相邻同角色 → 连续同角色合并（4.1）
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
                    // 刚 push 的 tool 消息与后续 text 块生成的 user 消息角色不同，无需合并
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

// ─────────────────────────── 响应：OpenAI → Anthropic ───────────────────────────

// 非流式：OpenAI Chat JSON → Anthropic Messages JSON（4.2）
pub fn convert_response(resp: &Value, src_model: &str) -> Value {
    let mut content: Vec<Value> = Vec::new();
    if let Some(msg) = resp.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message")) {
        if let Some(t) = msg.get("content").and_then(Value::as_str) {
            if !t.is_empty() {
                content.push(json!({"type": "text", "text": t}));
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

fn map_usage(u: Option<&Value>) -> Value {
    let input = u.and_then(|u| u.get("prompt_tokens")).and_then(Value::as_u64).unwrap_or(0);
    let output = u.and_then(|u| u.get("completion_tokens")).and_then(Value::as_u64).unwrap_or(0);
    json!({"input_tokens": input, "output_tokens": output})
}

fn map_stop_reason(r: &str) -> &'static str {
    match r {
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        _ => "end_turn",
    }
}

// 上游（openai）错误体 → Anthropic 错误结构，保留状态码（4.4）
// （入口在 proxy.rs::upstream_error_response，此处仅导出类型映射供其复用）

// ─────────────────────────── 流式：OpenAI chunks → Anthropic SSE ───────────────────────────
// 状态机维护「当前开着的 block」，保证 start/stop 配对——CLI 对未闭合 block 会报错（4.2）

#[derive(PartialEq)]
enum OpenBlock {
    None,
    Text,
    Tool(usize), // OpenAI tool_calls 的 index
}

pub struct StreamTranslator {
    src_model: String,
    block_index: u64,     // 已分配的 Anthropic content block 序号
    open: OpenBlock,      // 当前开着的 block
    emitted_any: bool,    // 是否产出过任何 content（空流兜底用）
    output_tokens: u64,
}

impl StreamTranslator {
    pub fn new(src_model: String) -> Self {
        Self { src_model, block_index: 0, open: OpenBlock::None, emitted_any: false, output_tokens: 0 }
    }
    // 流开头：message_start（content 尚为空，块随转换逐个出现）
    pub fn start_events(&self) -> Vec<Value> {
        vec![json!({
            "type": "message_start",
            "message": {
                "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
                "type": "message",
                "role": "assistant",
                "model": self.src_model,
                "content": [],
                "stop_reason": Value::Null,
                "usage": {"input_tokens": 0, "output_tokens": 0},
            }
        })]
    }

    // 喂一个 OpenAI chunk（data: JSON 部分），产出 0..n 个 Anthropic SSE 事件
    pub fn feed(&mut self, chunk: &Value) -> Vec<Value> {
        let mut events = Vec::new();
        let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) else {
            // 带 usage 的终止 chunk（stream_options 时）：usage 累计，无内容事件
            if let Some(u) = chunk.get("usage") {
                self.output_tokens = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(self.output_tokens);
            }
            return events;
        };

        if let Some(delta) = choice.get("delta") {
            // 文本增量
            if let Some(t) = delta.get("content").and_then(Value::as_str) {
                if !t.is_empty() {
                    if self.open != OpenBlock::Text {
                        self.close_block(&mut events);
                        events.push(self.block_start(json!({"type": "text", "text": ""})));
                        self.open = OpenBlock::Text;
                    }
                    self.emitted_any = true;
                    events.push(json!({
                        "type": "content_block_delta",
                        "index": self.block_index - 1,
                        "delta": {"type": "text_delta", "text": t},
                    }));
                }
            }
            // delta.reasoning_content（部分上游的思考字段）丢弃

            // 工具调用分片：按 index 归组，首次出片开块，后续续片
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let idx = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    if self.open != OpenBlock::Tool(idx) {
                        self.close_block(&mut events);
                        let name = call["function"]["name"].as_str().unwrap_or("");
                        let id = call.get("id").and_then(Value::as_str).filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("callu_{:08x}", self.block_index));
                        events.push(self.block_start(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": {},
                        })));
                        self.open = OpenBlock::Tool(idx);
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        if !args.is_empty() {
                            self.emitted_any = true;
                            events.push(json!({
                                "type": "content_block_delta",
                                "index": self.block_index - 1,
                                "delta": {"type": "input_json_delta", "partial_json": args},
                            }));
                        }
                    }
                }
            }
        }

        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.close_block(&mut events);
            self.output_tokens = choice
                .get("usage")
                .and_then(|u| u.get("completion_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or(self.output_tokens);
            let _ = fr; // stop_reason 在 finish() 里统一发出
        }
        events
    }

    // 流结束：空流补空 text block 兜底 + message_delta(stop_reason/usage) + message_stop
    pub fn finish(&mut self, stop_reason: &str) -> Vec<Value> {
        let mut events = Vec::new();
        if !self.emitted_any {
            events.push(self.block_start(json!({"type": "text", "text": ""})));
            events.push(json!({
                "type": "content_block_delta",
                "index": self.block_index - 1,
                "delta": {"type": "text_delta", "text": ""},
            }));
        }
        self.close_block(&mut events);
        events.push(json!({
            "type": "message_delta",
            "delta": {"stop_reason": map_stop_reason(stop_reason), "stop_sequence": Value::Null},
            "usage": {"output_tokens": self.output_tokens},
        }));
        events.push(json!({"type": "message_stop"}));
        events
    }

    fn block_start(&mut self, content_block: Value) -> Value {
        let ev = json!({
            "type": "content_block_start",
            "index": self.block_index,
            "content_block": content_block,
        });
        self.block_index += 1;
        ev
    }

    fn close_block(&mut self, events: &mut Vec<Value>) {
        if self.open != OpenBlock::None {
            events.push(json!({"type": "content_block_stop", "index": self.block_index - 1}));
            self.open = OpenBlock::None;
        }
    }
}
// SSE 帧解析辅助：从缓冲区拆出「已完整」（以换行结尾）的 data: 行。
// 返回 (data 负载列表, 已消费字节数)；不完整的尾行保留在缓冲区等下一块。
// [DONE] 由调用方按返回的 String 判断。
pub fn parse_sse_frames(buf: &str) -> (Vec<String>, usize) {
    let mut events = Vec::new();
    let mut consumed = 0;
    for line in buf.split_inclusive('\n') {
        if !line.ends_with('\n') {
            break; // 尾行不完整：留给后续字节拼
        }
        if let Some(data) = line.trim_end_matches(['\n', '\r']).strip_prefix("data:") {
            events.push(data.trim_start().to_string());
        }
        consumed += line.len();
    }
    (events, consumed)
}

// [DONE] 标记对应的 finish_reason
pub const DONE_SENTINEL: &str = "[DONE]";
