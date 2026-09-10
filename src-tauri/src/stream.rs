// OpenAI Chat Completions SSE → Anthropic Messages SSE 翻译状态机
//
// 关键约束（客户端 Claude Code 会校验）：
//   · content_block_start / stop 必须严格配对，未闭合的块会让客户端报错
//   · message_delta 每个流只能发一次，且要在拿到完整 usage 之后（延迟到流尾）
//   · 工具块必须等 id 与 name 都到齐才能开——首片常常只有 name 或只有 args
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::convert::{map_stop_reason, map_usage};

// ─────────────────────── 内部标签净化（压缩摘要请求） ───────────────────────
//
// Claude Code 上下文压缩时会发「总结对话」请求，提示词要求模型把思考写进
// <analysis>、总结写进 <summary>。官方链路里由客户端自己剥离这两个标签
// （claude.exe 的 ihg()：删掉 analysis 段、取 summary 正文）；
// 走 openai 转换时若原样透传，标签就会显示在可见输出里。
// 这里对齐同款语义：丢弃 analysis 段，summary 只留正文。
// 不改成 thinking 块——客户端会解析该标签，块类型一变反而可能让压缩校验失败。
const ANALYSIS_OPEN: &str = "<analysis>";
const ANALYSIS_CLOSE: &str = "</analysis>";
const SUMMARY_TAGS: [&str; 2] = ["<summary>", "</summary>"];

#[derive(PartialEq)]
enum TagPhase {
    // 还看不出正文是否以 <analysis> 开头，先攒着
    Detecting,
    // 正在 analysis 段内，内容丢弃（仅留作空输出兜底）
    Analysis,
    // 普通正文，只需剥掉 summary 标签
    Body,
}

pub struct TagSanitizer {
    phase: TagPhase,
    buf: String,
    // analysis 段内容：正常丢弃，仅当整条消息没有任何正文时兜底吐出
    dropped: String,
    emitted: bool,
}

impl Default for TagSanitizer {
    fn default() -> Self {
        Self {
            phase: TagPhase::Detecting,
            buf: String::new(),
            dropped: String::new(),
            emitted: false,
        }
    }
}

impl TagSanitizer {
    // 喂一段正文增量，返回可输出的 text 增量
    pub fn push(&mut self, chunk: &str) -> String {
        // 一律走缓冲区：Body 阶段也要留住尾部可能的半截标签，否则会被切碎漏出去
        self.buf.push_str(chunk);
        self.drain(false)
    }

    // 流结束：吐出攒着的尾巴；全程只有 analysis 段时兜底返回它，避免空消息
    pub fn finish(&mut self) -> String {
        let mut out = self.drain(true);
        if !self.emitted && out.is_empty() && !self.dropped.trim().is_empty() {
            out = strip_summary_tags(&self.dropped);
        }
        out
    }

    fn drain(&mut self, at_end: bool) -> String {
        let mut text = String::new();
        loop {
            match self.phase {
                TagPhase::Detecting => {
                    let lead = self.buf.len() - self.buf.trim_start().len();
                    let rest = &self.buf[lead..];
                    if rest.starts_with(ANALYSIS_OPEN) {
                        self.buf.drain(..lead + ANALYSIS_OPEN.len());
                        self.phase = TagPhase::Analysis;
                    } else if ANALYSIS_OPEN.starts_with(rest) && !at_end {
                        return text; // 可能是 analysis 前缀，等下一片
                    } else {
                        self.phase = TagPhase::Body;
                    }
                }
                TagPhase::Analysis => match self.buf.find(ANALYSIS_CLOSE) {
                    Some(i) => {
                        self.dropped.push_str(&self.buf[..i]);
                        self.buf.drain(..i + ANALYSIS_CLOSE.len());
                        self.phase = TagPhase::Body;
                    }
                    None => {
                        if at_end {
                            self.dropped.push_str(&std::mem::take(&mut self.buf));
                        }
                        return text;
                    }
                },
                TagPhase::Body => {
                    // 尾部可能是被切断的标签前缀，留到下一片；流已结束则不再留
                    let hold = if at_end { 0 } else { partial_tag_suffix_len(&self.buf) };
                    let cut = self.buf.len() - hold;
                    let chunk: String = self.buf.drain(..cut).collect();
                    text.push_str(&strip_summary_tags(&chunk));
                    if !text.is_empty() {
                        self.emitted = true;
                    }
                    return text;
                }
            }
        }
    }
}

// 结尾有多少字节是某个标签的**不完整**前缀，需留到下一片再判；
// 完整标签不算（交给 strip_summary_tags 直接剥掉）
fn partial_tag_suffix_len(s: &str) -> usize {
    let b = s.as_bytes();
    let max = SUMMARY_TAGS.iter().map(|t| t.len()).max().unwrap_or(0);
    for k in (1..max).rev() {
        if k > b.len() {
            continue;
        }
        let tail = &b[b.len() - k..];
        if SUMMARY_TAGS.iter().any(|t| k < t.len() && t.as_bytes().starts_with(tail)) {
            return k;
        }
    }
    0
}

fn strip_summary_tags(s: &str) -> String {
    let mut out = s.to_string();
    for tag in SUMMARY_TAGS {
        if out.contains(tag) {
            out = out.replace(tag, "");
        }
    }
    out
}

// 非流式正文：一次性净化
pub fn sanitize_internal_tags(raw: &str) -> String {
    let mut s = TagSanitizer::default();
    let mut out = s.push(raw);
    out.push_str(&s.finish());
    out
}

// ─────────────────────────── SSE 帧读写 ───────────────────────────

pub const DONE_SENTINEL: &str = "[DONE]";

// 从缓冲区拆出「已完整」（以空行分隔）的 data 负载；返回 (负载列表, 已消费字节数)。
// 不完整的尾块保留在缓冲区等下一批字节。同时兼容 \n\n 与 \r\n\r\n。
pub fn parse_sse_frames(buf: &str) -> (Vec<String>, usize) {
    let mut events = Vec::new();
    let mut consumed = 0;
    loop {
        let rest = &buf[consumed..];
        let end = ["\r\n\r\n", "\n\n"]
            .iter()
            .filter_map(|d| rest.find(d).map(|p| (p, d.len())))
            .min_by_key(|(p, _)| *p);
        let Some((pos, dlen)) = end else { break };
        for line in rest[..pos].lines() {
            if let Some(data) = strip_sse_field(line, "data") {
                events.push(data.trim_start().to_string());
            }
        }
        consumed += pos + dlen;
    }
    (events, consumed)
}

fn strip_sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(&format!("{field}: "))
        .or_else(|| line.strip_prefix(&format!("{field}:")))
}

// 把原始字节追加到 String 缓冲，正确处理跨块截断的多字节 UTF-8。
// remainder 暂存上一块末尾不完整的序列（最多 3 字节）；直接 from_utf8 会丢掉整个流
pub fn append_utf8_safe(buffer: &mut String, remainder: &mut Vec<u8>, new_bytes: &[u8]) {
    let combined;
    let input: &[u8] = if remainder.is_empty() {
        new_bytes
    } else {
        let mut v = std::mem::take(remainder);
        v.extend_from_slice(new_bytes);
        combined = v;
        &combined
    };
    let mut pos = 0;
    loop {
        match std::str::from_utf8(&input[pos..]) {
            Ok(s) => {
                buffer.push_str(s);
                return;
            }
            Err(e) => {
                let valid_up_to = pos + e.valid_up_to();
                buffer.push_str(&String::from_utf8_lossy(&input[pos..valid_up_to]));
                match e.error_len() {
                    // 真非法字节：输出替换符后继续
                    Some(len) => {
                        buffer.push('\u{FFFD}');
                        pos = valid_up_to + len;
                    }
                    // 尾部不完整序列：留到下一块
                    None => {
                        *remainder = input[valid_up_to..].to_vec();
                        return;
                    }
                }
            }
        }
    }
}

// Anthropic SSE 事件序列 → 响应字节（event: 类型 + data: JSON + 空行）
pub fn render_sse(events: &[Value]) -> String {
    let mut out = String::new();
    for ev in events {
        out.push_str(&format!(
            "event: {}\ndata: {}\n\n",
            ev["type"].as_str().unwrap_or("message"),
            ev
        ));
    }
    out
}

// ─────────────────────────── 翻译状态机 ───────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum OpenBlock {
    None,
    Text,
    Thinking,
}

// 一个 OpenAI tool_call 分片的累积状态；anthropic_index 在首次出现时分配
struct ToolState {
    index: u64,
    id: String,
    name: String,
    started: bool,
    pending: String,
}

pub struct StreamTranslator {
    src_model: String,
    started: bool,
    block_index: u64,
    open: OpenBlock,
    tools: BTreeMap<usize, ToolState>,
    open_tools: Vec<u64>,
    usage: Value,
    stop_reason: Option<String>,
    // 正文里的 <analysis>/<summary> 标签净化器（压缩摘要请求会带这些标签）
    sanitizer: TagSanitizer,
}

impl StreamTranslator {
    pub fn new(src_model: String) -> Self {
        Self {
            src_model,
            started: false,
            block_index: 0,
            open: OpenBlock::None,
            tools: BTreeMap::new(),
            open_tools: Vec::new(),
            usage: json!({"input_tokens": 0, "output_tokens": 0}),
            stop_reason: None,
            sanitizer: TagSanitizer::default(),
        }
    }

    // 喂一个 OpenAI chunk（data: JSON 部分），产出 0..n 个 Anthropic SSE 事件
    pub fn feed(&mut self, chunk: &Value) -> Vec<Value> {
        let mut events = Vec::new();
        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            self.usage = map_usage(Some(u));
        }
        let Some(choice) = chunk.get("choices").and_then(|c| c.get(0)) else {
            return events; // 仅带 usage 的终止块
        };
        if !self.started {
            self.started = true;
            events.push(self.message_start(chunk));
        }
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(map_stop_reason(fr).to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return events;
        };

        // 思考增量（DeepSeek 用 reasoning_content，OpenRouter/Kimi 用 reasoning）
        let reasoning = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str);
        if let Some(t) = reasoning.filter(|t| !t.is_empty()) {
            self.open_non_tool(OpenBlock::Thinking, &mut events);
            events.push(delta_event(
                self.block_index - 1,
                json!({"type": "thinking_delta", "thinking": t}),
            ));
        }

        if let Some(t) = delta.get("content").and_then(Value::as_str) {
            if !t.is_empty() {
                // 压缩摘要请求的正文带 <analysis>/<summary> 标签：丢弃 analysis 段、
                // 剥掉 summary 标签——原样透传会让标签出现在可见输出里
                let text = self.sanitizer.push(t);
                if !text.is_empty() {
                    self.open_non_tool(OpenBlock::Text, &mut events);
                    events.push(delta_event(
                        self.block_index - 1,
                        json!({"type": "text_delta", "text": text}),
                    ));
                }
            }
        }

        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            if !calls.is_empty() {
                self.close_non_tool(&mut events);
                for call in calls {
                    self.feed_tool_call(call, &mut events);
                }
            }
        }
        events
    }

    // 流结束：补空块兜底 + 未开完的工具块 late start + 收尾事件。
    // message_delta 在此统一发出（只发一次，且此时 usage 已完整）
    pub fn finish(&mut self) -> Vec<Value> {
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(self.message_start(&Value::Null));
        }
        // 净化器里可能还攒着未判定的尾巴（如截断的标签前缀），先吐干净
        let text = self.sanitizer.finish();
        if !text.is_empty() {
            self.open_non_tool(OpenBlock::Text, &mut events);
            events.push(delta_event(
                self.block_index - 1,
                json!({"type": "text_delta", "text": text}),
            ));
        }
        self.close_non_tool(&mut events);
        self.late_start_tools(&mut events);
        // 全程没有任何内容块（空流/纯 usage）→ 补一个空 text 块，客户端要求至少一个块
        if self.tools.is_empty() && self.block_index == 0 {
            events.push(self.block_start(json!({"type": "text", "text": ""})));
            events.push(delta_event(0, json!({"type": "text_delta", "text": ""})));
            events.push(json!({"type": "content_block_stop", "index": 0}));
        }
        for index in std::mem::take(&mut self.open_tools) {
            events.push(json!({"type": "content_block_stop", "index": index}));
        }
        let stop = self.stop_reason.clone().unwrap_or_else(|| "end_turn".to_string());
        events.push(json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop, "stop_sequence": Value::Null},
            "usage": self.usage,
        }));
        events.push(json!({"type": "message_stop"}));
        events
    }

    // ── 内部：块管理 ──

    fn message_start(&self, chunk: &Value) -> Value {
        let id = chunk
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("msg_{}", uuid::Uuid::new_v4().simple()));
        let model = chunk
            .get("model")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.src_model);
        json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": Value::Null,
                "usage": self.usage,
            }
        })
    }

    // 开一个非工具块；类型相同则续用，不同则先关旧的
    fn open_non_tool(&mut self, kind: OpenBlock, events: &mut Vec<Value>) {
        if self.open == kind {
            return;
        }
        self.close_non_tool(events);
        let block = match kind {
            OpenBlock::Thinking => json!({"type": "thinking", "thinking": ""}),
            _ => json!({"type": "text", "text": ""}),
        };
        events.push(self.block_start(block));
        self.open = kind;
    }

    fn close_non_tool(&mut self, events: &mut Vec<Value>) {
        if self.open != OpenBlock::None {
            events.push(json!({"type": "content_block_stop", "index": self.block_index - 1}));
            self.open = OpenBlock::None;
        }
    }

    // 一个工具分片：按 index 归组；id+name 齐了才 start，之前先把 args 攒着
    fn feed_tool_call(&mut self, call: &Value, events: &mut Vec<Value>) {
        let idx = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        // 首次见到该工具分片时分配 Anthropic block 序号（与文本块共用同一计数器）
        if !self.tools.contains_key(&idx) {
            let index = self.block_index;
            self.block_index += 1;
            self.tools.insert(
                idx,
                ToolState {
                    index,
                    id: String::new(),
                    name: String::new(),
                    started: false,
                    pending: String::new(),
                },
            );
        }
        let state = self.tools.get_mut(&idx).expect("just inserted");
        if let Some(id) = call.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            state.id = id.to_string();
        }
        if let Some(name) = call["function"]["name"].as_str().filter(|s| !s.is_empty()) {
            state.name = name.to_string();
        }

        let args = call["function"]["arguments"].as_str().unwrap_or("");
        if !state.started {
            // 首片常常只带 name、id 或 args 之一，凑齐前不能开块
            if state.id.is_empty() || state.name.is_empty() {
                state.pending.push_str(args);
                return;
            }
            state.started = true;
            let (index, id, name, pending) = (
                state.index,
                state.id.clone(),
                state.name.clone(),
                std::mem::take(&mut state.pending),
            );
            events.push(json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}},
            }));
            self.open_tools.push(index);
            let tail = format!("{pending}{args}");
            if !tail.is_empty() {
                events.push(delta_event(
                    index,
                    json!({"type": "input_json_delta", "partial_json": tail}),
                ));
            }
        } else if !args.is_empty() {
            let index = state.index;
            events.push(delta_event(
                index,
                json!({"type": "input_json_delta", "partial_json": args}),
            ));
        }
    }

    // 流结束仍未凑齐 id/name 的工具块：用兜底值补开，保证 start/stop 配对
    fn late_start_tools(&mut self, events: &mut Vec<Value>) {
        let mut late: Vec<(u64, String, String, String)> = Vec::new();
        for (idx, st) in self.tools.iter_mut() {
            if st.started {
                continue;
            }
            if st.pending.is_empty() && st.id.is_empty() && st.name.is_empty() {
                continue;
            }
            st.started = true;
            let id = if st.id.is_empty() { format!("tool_call_{idx}") } else { st.id.clone() };
            let name = if st.name.is_empty() { "unknown_tool".to_string() } else { st.name.clone() };
            late.push((st.index, id, name, std::mem::take(&mut st.pending)));
        }
        late.sort_unstable_by_key(|(i, _, _, _)| *i);
        for (index, id, name, pending) in late {
            events.push(json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}},
            }));
            self.open_tools.push(index);
            if !pending.is_empty() {
                events.push(delta_event(
                    index,
                    json!({"type": "input_json_delta", "partial_json": pending}),
                ));
            }
        }
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
}

fn delta_event(index: u64, delta: Value) -> Value {
    json!({"type": "content_block_delta", "index": index, "delta": delta})
}

// 完整 Anthropic message JSON → 单轮 SSE 事件序列（上游对 stream:true 回了 JSON 的兜底）
pub fn full_message_to_sse(msg: &Value) -> Vec<Value> {
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
        events.push(delta_event(i as u64, delta));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frames_handles_lf_and_crlf() {
        let (ev, used) = parse_sse_frames("data: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert_eq!(ev, vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
        assert_eq!(used, "data: {\"a\":1}\n\ndata: [DONE]\n\n".len());
        // CRLF 分隔同样识别
        let (ev, used) = parse_sse_frames("data: {\"a\":1}\r\n\r\ndata: [DONE]\r\n\r\n");
        assert_eq!(ev, vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
        assert_eq!(used, "data: {\"a\":1}\r\n\r\ndata: [DONE]\r\n\r\n".len());
        // 尾块不完整：留给下一批
        let (ev, used) = parse_sse_frames("data: {\"a\":1}\n\ndata: {\"b\"");
        assert_eq!(ev, vec!["{\"a\":1}".to_string()]);
        assert_eq!(used, 15);
    }

    #[test]
    fn utf8_split_across_chunks_is_reassembled() {
        let bytes = "你".as_bytes();
        let mut buf = String::new();
        let mut rem = Vec::new();
        append_utf8_safe(&mut buf, &mut rem, &bytes[..2]);
        assert_eq!(buf, "");
        append_utf8_safe(&mut buf, &mut rem, &bytes[2..]);
        assert_eq!(buf, "你");
        assert!(rem.is_empty());
    }

    // 首片只有 name、第二片才带 id 与 args —— 必须等齐再开块，args 不能丢
    #[test]
    fn tool_block_waits_for_id_and_name() {
        let mut t = StreamTranslator::new("claude-custom".into());
        let mut ev = t.feed(&json!({
            "id": "c1", "model": "m",
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"name": "Read"}}
            ]}}]
        }));
        assert!(ev.iter().all(|e| e["type"] != "content_block_start"));
        ev = t.feed(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_1", "function": {"arguments": "{\"a\""}}
            ]}}]
        }));
        let starts: Vec<_> = ev.iter().filter(|e| e["type"] == "content_block_start").collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["content_block"]["name"], "Read");
        assert_eq!(starts[0]["content_block"]["id"], "call_1");
        let deltas: Vec<_> = ev.iter().filter(|e| e["type"] == "content_block_delta").collect();
        assert_eq!(deltas[0]["delta"]["partial_json"], "{\"a\"");
    }

    // 攒了 args 但 id 始终没来 → late start 兜底，且 start/stop 必须配对
    #[test]
    fn late_start_closes_unfinished_tool_block() {
        let mut t = StreamTranslator::new("m".into());
        t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]}}]
        }));
        let ev = t.finish();
        let starts = ev.iter().filter(|e| e["type"] == "content_block_start").count();
        let stops = ev.iter().filter(|e| e["type"] == "content_block_stop").count();
        assert_eq!(starts, 1);
        assert_eq!(stops, 1);
    }

    #[test]
    fn message_delta_emitted_once_with_usage() {
        let mut t = StreamTranslator::new("m".into());
        t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"content": "hi"}, "finish_reason": "stop"}]
        }));
        t.feed(&json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 7}}));
        let ev = t.finish();
        let deltas: Vec<_> = ev.iter().filter(|e| e["type"] == "message_delta").collect();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["delta"]["stop_reason"], "end_turn");
        assert_eq!(deltas[0]["usage"]["input_tokens"], 100);
        assert_eq!(deltas[0]["usage"]["output_tokens"], 7);
    }

    // prompt_tokens 含缓存，Anthropic 的 input_tokens 必须扣掉，否则重复计费
    #[test]
    fn cache_tokens_are_subtracted_from_input() {
        let mut t = StreamTranslator::new("m".into());
        t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"content": "x"}}],
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 5,
                "prompt_tokens_details": {"cached_tokens": 800, "cache_write_tokens": 100}
            }
        }));
        let ev = t.finish();
        let d = ev.iter().find(|e| e["type"] == "message_delta").unwrap();
        assert_eq!(d["usage"]["input_tokens"], 100);
        assert_eq!(d["usage"]["cache_read_input_tokens"], 800);
        assert_eq!(d["usage"]["cache_creation_input_tokens"], 100);
    }

    #[test]
    fn reasoning_maps_to_thinking_block() {
        let mut t = StreamTranslator::new("m".into());
        let ev = t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"reasoning_content": "想一下"}}]
        }));
        assert!(ev.iter().any(|e| e["content_block"]["type"] == "thinking"));
        let d = ev.iter().find(|e| e["type"] == "content_block_delta").unwrap();
        assert_eq!(d["delta"]["type"], "thinking_delta");
    }

    #[test]
    fn empty_stream_still_produces_valid_sequence() {
        let mut t = StreamTranslator::new("m".into());
        let ev = t.finish();
        assert_eq!(ev[0]["type"], "message_start");
        assert_eq!(ev[1]["content_block"]["type"], "text");
        assert_eq!(ev[ev.len() - 1]["type"], "message_stop");
    }

    // 多个并行工具调用：block 序号必须互不相同，否则客户端索引错乱
    #[test]
    fn parallel_tool_calls_get_distinct_block_indices() {
        let mut t = StreamTranslator::new("m".into());
        let mut ev = t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "a", "function": {"name": "Read", "arguments": "{}"}},
                {"index": 1, "id": "b", "function": {"name": "Bash", "arguments": "{}"}}
            ]}}]
        }));
        ev.extend(t.finish());
        let starts: Vec<u64> = ev
            .iter()
            .filter(|e| e["type"] == "content_block_start")
            .map(|e| e["index"].as_u64().unwrap())
            .collect();
        assert_eq!(starts.len(), 2);
        assert_ne!(starts[0], starts[1], "并行工具块的 index 不能重复");
    }

    // 压缩摘要请求：analysis 段丢弃，summary 只留正文，标签绝不进正文
    #[test]
    fn compact_summary_tags_never_reach_text() {
        let mut t = StreamTranslator::new("m".into());
        let mut ev = t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"content": "<analysis>\n思考内容\n</analysis>\n"}}]
        }));
        ev.extend(t.feed(&json!({
            "choices": [{"delta": {"content": "<summary>\n1. 主要请求\n</summary>"}}]
        })));
        ev.extend(t.finish());
        let text: String = ev.iter()
            .filter(|e| e["delta"]["type"] == "text_delta")
            .map(|e| e["delta"]["text"].as_str().unwrap())
            .collect();
        assert!(!text.contains("<analysis>") && !text.contains("<summary>"), "标签不能出现在正文: {text}");
        assert!(!text.contains("思考内容"), "analysis 段应丢弃");
        assert!(text.contains("1. 主要请求"), "summary 正文要保留");
    }

    // 标签被切成多个 SSE 分片：仍要正确识别，且不能丢字
    #[test]
    fn compact_tags_split_across_chunks_are_handled() {
        let mut t = StreamTranslator::new("m".into());
        let mut ev = t.feed(&json!({"id": "c1", "choices": [{"delta": {"content": "<ana"}}]}));
        ev.extend(t.feed(&json!({"choices": [{"delta": {"content": "lysis>想</anal"}}]})));
        ev.extend(t.feed(&json!({"choices": [{"delta": {"content": "ysis><summ"}}]})));
        ev.extend(t.feed(&json!({"choices": [{"delta": {"content": "ary>正文</summary>"}}]})));
        ev.extend(t.finish());
        let text: String = ev.iter()
            .filter(|e| e["delta"]["type"] == "text_delta")
            .map(|e| e["delta"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(text, "正文");
        assert!(!text.contains('<'));
    }

    // 普通正文（无标签）不能被净化逻辑吃掉或改动
    #[test]
    fn plain_text_passes_through_untouched() {
        let mut t = StreamTranslator::new("m".into());
        let mut ev = t.feed(&json!({
            "id": "c1", "choices": [{"delta": {"content": "看看 <code> 和 a<b 这种普通文本"}}]
        }));
        ev.extend(t.finish());
        let text: String = ev.iter()
            .filter(|e| e["delta"]["type"] == "text_delta")
            .map(|e| e["delta"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(text, "看看 <code> 和 a<b 这种普通文本");
    }

    // 只有 analysis 段、没有正文时兜底输出，避免整条消息变空
    #[test]
    fn analysis_only_still_yields_text() {
        let mut t = StreamTranslator::new("m".into());
        let mut ev = t.feed(&json!({
            "id": "c1", "choices": [{"delta": {"content": "<analysis>只有思考</analysis>"}}]
        }));
        ev.extend(t.finish());
        let text: String = ev.iter()
            .filter(|e| e["delta"]["type"] == "text_delta")
            .map(|e| e["delta"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(text, "只有思考");
    }

    #[test]
    fn sanitize_internal_tags_non_stream() {
        assert_eq!(
            sanitize_internal_tags("<analysis>想\n</analysis><summary>结论</summary>"),
            "结论"
        );
        // 无标签时原样返回
        assert_eq!(sanitize_internal_tags("普通回答"), "普通回答");
    }

    // 文本 + 工具混排：所有 start 必须有配对的 stop
    #[test]
    fn every_block_start_has_matching_stop() {
        let mut t = StreamTranslator::new("m".into());
        let mut ev = t.feed(&json!({
            "id": "c1",
            "choices": [{"delta": {"content": "看看文件"}}]
        }));
        ev.extend(t.feed(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "a", "function": {"name": "Read", "arguments": "{\"p\""}}
            ]}}]
        })));
        ev.extend(t.feed(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": ":1}"}}
            ]}}]
        })));
        ev.extend(t.finish());
        let starts: Vec<u64> = ev.iter().filter(|e| e["type"] == "content_block_start")
            .map(|e| e["index"].as_u64().unwrap()).collect();
        let stops: Vec<u64> = ev.iter().filter(|e| e["type"] == "content_block_stop")
            .map(|e| e["index"].as_u64().unwrap()).collect();
        assert_eq!(starts, stops, "每个 content_block_start 必须有同 index 的 stop");
        // 分片的 args 必须完整拼回
        let json: String = ev.iter()
            .filter(|e| e["delta"]["type"] == "input_json_delta")
            .map(|e| e["delta"]["partial_json"].as_str().unwrap())
            .collect();
        assert_eq!(json, "{\"p\":1}");
    }
}
