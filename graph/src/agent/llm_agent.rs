use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::memory::{Message, Role, ToolCall};
use crate::agent::tools::ToolsManager;

// ─── Memory compression constants ────────────────────────────────────────────

/// Fraction of the compressible range (everything except the system prompt and
/// the two most-recent messages) that gets folded into the summary.
const COMPRESSION_FRACTION: usize = 70; // percent

/// Approximate characters-per-token used when deriving thresholds.
const CHARS_PER_TOKEN: usize = 4;

/// Compress when the conversation uses this fraction of the model's context.
const COMPRESSION_AT_FRACTION: usize = 50; // percent

/// Return the context-window size (in tokens) for a given model name.
///
/// Uses heuristic substring matching (case-insensitive) — the name does not
/// need to be an exact API identifier.
pub fn context_window_for_model(model: &str) -> usize {
    let m = model.to_ascii_lowercase();

    // ── OpenAI GPT ────────────────────────────────────────────────────────────
    if m.contains("gpt-4.1") || m.contains("gpt4.1")            { return 1_047_576; }
    if m.contains("gpt-5")   || m.contains("gpt5")              { return   400_000; }
    if m.contains("gpt-4o")  || m.contains("gpt4o")             { return   128_000; }
    if m.contains("gpt-4")   || m.contains("gpt4")              { return   128_000; }
    if m.contains("gpt-3.5") || m.contains("gpt3.5")
                             || m.contains("gpt-3")              { return    16_385; }
    // ── OpenAI o-series ───────────────────────────────────────────────────────
    if m.contains("o1") || m.contains("o3") || m.contains("o4") { return   200_000; }
    // ── Anthropic Claude ──────────────────────────────────────────────────────
    // Sonnet 4.5+, Opus 4.5+ and all 4.6+ models support 1M extended context.
    // Claude 3.x and 4.0–4.4 are 200k.
    if m.contains("claude") && (m.contains("4.6") || m.contains("4-6")) { return 1_000_000; }
    if m.contains("claude") && (m.contains("4.5") || m.contains("4-5")) { return 1_000_000; }
    if m.contains("claude")                                              { return   200_000; }
    // ── Google Gemini ─────────────────────────────────────────────────────────
    if m.contains("gemini-2") || m.contains("gemini2")          { return 1_048_000; }
    if m.contains("gemini")                                      { return   128_000; }
    if m.contains("gemma")                                       { return   128_000; }
    // ── DeepSeek ──────────────────────────────────────────────────────────────
    if m.contains("deepseek")                                    { return    64_000; }
    // ── Qwen ──────────────────────────────────────────────────────────────────
    if m.contains("qwen")                                        { return   131_072; }
    // ── Mistral / Mixtral ─────────────────────────────────────────────────────
    if m.contains("mistral-nemo") || m.contains("mistralnemo")  { return   128_000; }
    if m.contains("mistral") || m.contains("mixtral")           { return    32_000; }
    // ── Meta Llama ────────────────────────────────────────────────────────────
    if m.contains("llama")                                       { return   131_072; }
    // ── Microsoft Phi ─────────────────────────────────────────────────────────
    if m.contains("phi-4") || m.contains("phi4")                { return    16_000; }

    // Default: assume a 128k context (conservative but widely applicable).
    128_000
}

/// Convert a context-window token count to a character threshold at which
/// compression should be triggered (50 % of context, 4 chars / token).
fn compression_threshold(context_tokens: usize) -> usize {
    context_tokens * COMPRESSION_AT_FRACTION / 100 * CHARS_PER_TOKEN
}

// ─── Agent action ─────────────────────────────────────────────────────────────

/// What TypeScript should do after calling `process_response`.
pub enum AgentAction {
    /// Tool calls were executed; call `get_request` and send another LLM turn.
    Continue,
    /// The agent produced a final text response.  Save it and stop.
    AssistantMessage(String),
    /// The agent called the `ask_questions` tool.  The payload is the raw JSON
    /// arguments string (contains a `questions` array).  The loop should stop
    /// and surface the questions to the user.
    AskQuestions(String),
    /// The LLM stopped without content (e.g. empty stop).
    Stop,
    /// A fatal error occurred; the string describes what went wrong.
    Error(String),
}

// ─── Wire types (LLM response shape) ──────────────────────────────────────────

/// Minimal subset of an OpenAI chat-completion response needed by the agent.
#[derive(Deserialize)]
struct LLMResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message:       AssistantMsg,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct AssistantMsg {
    content:    Option<String>,
    tool_calls: Option<Vec<ToolCallRaw>>,
}

#[derive(Deserialize)]
struct ToolCallRaw {
    id:       String,
    function: ToolFn,
}

#[derive(Deserialize)]
struct ToolFn {
    name:      String,
    arguments: String,
}

// ─── OpenAI message wire shape ────────────────────────────────────────────────
//
// The messages we send to the LLM must follow OpenAI's schema exactly.
// Our internal `Message` struct uses a flat layout; we need a richer
// representation for assistant messages that carry tool_calls.

#[derive(Serialize)]
#[serde(untagged)]
enum WireMessage {
    Plain {
        role:    String,
        content: String,
    },
    AssistantWithTools {
        role:       String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content:    Option<String>,
        tool_calls: Vec<WireToolCall>,
    },
    ToolResult {
        role:         String,
        tool_call_id: String,
        content:      String,
    },
}

#[derive(Serialize)]
struct WireToolCall {
    id:       String,
    #[serde(rename = "type")]
    kind:     String,
    function: WireToolFn,
}

#[derive(Serialize)]
struct WireToolFn {
    name:      String,
    arguments: String,
}

fn to_wire(msg: &Message) -> WireMessage {
    match msg.role {
        Role::Tool => WireMessage::ToolResult {
            role:         "tool".into(),
            tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
            content:      msg.content.clone().unwrap_or_default(),
        },
        Role::Assistant if !msg.tool_calls.is_empty() => WireMessage::AssistantWithTools {
            role:    "assistant".into(),
            content: msg.content.clone(),
            tool_calls: msg.tool_calls.iter().map(|tc| WireToolCall {
                id:       tc.id.clone(),
                kind:     "function".into(),
                function: WireToolFn {
                    name:      tc.name.clone(),
                    arguments: tc.arguments.clone(),
                },
            }).collect(),
        },
        _ => WireMessage::Plain {
            role:    format!("{:?}", msg.role).to_lowercase(),
            content: msg.content.clone().unwrap_or_default(),
        },
    }
}

// ─── LLM request shape ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct LLMRequest<'a> {
    messages: Vec<WireMessage>,
    tools:    Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    model:    Option<&'a str>,
}

// ─── LLMAgent ─────────────────────────────────────────────────────────────────

pub struct LLMAgent {
    pub messages:      Vec<Message>,
    pub tools_manager: ToolsManager,
    /// True while we are waiting for the LLM to return a compression summary.
    compressing:       bool,
    /// Exclusive end index of the messages slice being compressed.
    compression_end:   usize,
    /// Character threshold at which compression is triggered.
    threshold_chars:   usize,
}

impl LLMAgent {
    pub fn new(messages: Vec<Message>, tools_manager: ToolsManager, model_name: &str) -> Self {
        let ctx = context_window_for_model(model_name);
        Self {
            messages,
            tools_manager,
            compressing:     false,
            compression_end: 0,
            threshold_chars: compression_threshold(ctx),
        }
    }

    /// Serialize the current conversation + tools into a JSON request that
    /// TypeScript should forward to the LLM.
    ///
    /// If the conversation has grown past `COMPRESSION_THRESHOLD_CHARS` this
    /// transparently returns a compression request instead.  TypeScript simply
    /// forwards it to the LLM; when the summary comes back via
    /// `process_response` the compressed messages are replaced and
    /// `AgentAction::Continue` is returned so the normal loop resumes.
    pub fn get_request(&mut self) -> String {
        // Start a compression pass if the memory is large enough.
        if !self.compressing && self.should_compress() {
            self.start_compression();
        }

        if self.compressing {
            return self.build_compression_request();
        }

        let request = LLMRequest {
            messages: self.messages.iter().map(to_wire).collect(),
            tools:    self.tools_manager.to_openai_tools(),
            model:    None,
        };
        serde_json::to_string(&request).unwrap_or_else(|e| {
            format!(r#"{{"error":"failed to serialize request: {}"}}"#, e)
        })
    }

    // ── Compression helpers ───────────────────────────────────────────────────

    fn total_chars(&self) -> usize {
        self.messages.iter().map(|m| m.char_count()).sum()
    }

    fn should_compress(&self) -> bool {
        // Need at least system + 2 compressible + 2 protected tail messages.
        self.messages.len() >= 5 && self.total_chars() > self.threshold_chars
    }

    /// Determine how many messages to compress and mark `self.compressing`.
    fn start_compression(&mut self) {
        let n = self.messages.len();
        // Protected: system prompt (index 0) and the last 2 messages.
        let safe_end = n.saturating_sub(2).max(1);
        if safe_end <= 1 { return; } // nothing to compress

        // Accumulate chars from index 1 until we hit COMPRESSION_FRACTION %.
        let compressible_chars: usize = self.messages[1..safe_end]
            .iter()
            .map(|m| m.char_count())
            .sum();
        let target = compressible_chars * COMPRESSION_FRACTION / 100;

        let mut cumulative = 0usize;
        let mut boundary   = 2usize; // exclusive end; minimum = 2 (compress at least msg[1])
        for (i, msg) in self.messages[1..safe_end].iter().enumerate() {
            cumulative += msg.char_count();
            boundary   = i + 2; // i is 0-based within the slice, so +2 for the slice offset
            if cumulative >= target { break; }
        }

        self.compression_end = boundary;
        self.compressing     = true;
    }

    /// Build a plain LLM request (no tools) that asks for a summary of the
    /// messages in `messages[1..compression_end]`.
    fn build_compression_request(&self) -> String {
        let mut history = String::new();
        for msg in &self.messages[1..self.compression_end] {
            match &msg.role {
                Role::User => {
                    history.push_str("[User]: ");
                    history.push_str(msg.content.as_deref().unwrap_or(""));
                    history.push('\n');
                }
                Role::Assistant => {
                    if !msg.tool_calls.is_empty() {
                        history.push_str("[Assistant called tools]: ");
                        let names: Vec<&str> = msg.tool_calls.iter()
                            .map(|tc| tc.name.as_str())
                            .collect();
                        history.push_str(&names.join(", "));
                        history.push('\n');
                    } else {
                        history.push_str("[Assistant]: ");
                        history.push_str(msg.content.as_deref().unwrap_or(""));
                        history.push('\n');
                    }
                }
                Role::Tool => {
                    history.push_str("[Tool result]: ");
                    history.push_str(msg.content.as_deref().unwrap_or(""));
                    history.push('\n');
                }
                Role::System => {}
            }
            history.push('\n');
        }

        let system = "You are a memory compression assistant for an AI coding agent. \
            Summarize the following conversation history into a single dense reference message. \
            Preserve every file path that was explored, key findings about code structure, \
            architectural patterns, important data, and any technical details discovered. \
            Be specific and complete — nothing important should be lost.";

        let user = format!(
            "Compress the following agent conversation history into a single summary:\n\n{}",
            history
        );

        serde_json::to_string(&serde_json::json!({
            "messages": [
                {"role": "system", "content": system},
                {"role": "user",   "content": user},
            ]
        }))
        .unwrap_or_default()
    }

    /// Apply the compression summary returned by the LLM.
    fn apply_compression(&mut self, summary: String) {
        let summary_msg = Message::user(format!(
            "[Compressed conversation history — {} messages summarised]\n\n{}",
            self.compression_end - 1,
            summary,
        ));
        // Replace messages[1..compression_end] with the summary.
        self.messages.drain(1..self.compression_end);
        self.messages.insert(1, summary_msg);

        self.compressing     = false;
        self.compression_end = 0;
    }

    /// Process a raw OpenAI-compatible LLM response JSON string.
    ///
    /// - If the LLM returned tool calls: execute them, append all results to
    ///   `messages`, and return `AgentAction::Continue`.
    /// - If the LLM produced a final text answer: return
    ///   `AgentAction::AssistantMessage(text)`.
    /// - Otherwise: return `AgentAction::Stop` or `AgentAction::Error`.
    pub fn process_response(&mut self, response_json: &str) -> AgentAction {
        // ── Handle compression response ───────────────────────────────────────
        if self.compressing {
            let resp: LLMResponse = match serde_json::from_str(response_json) {
                Ok(r)  => r,
                Err(_) => {
                    // Compression failed — just clear the flag and continue.
                    self.compressing = false;
                    return AgentAction::Continue;
                }
            };
            let summary = resp.choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "[compression produced no summary]".to_owned());
            self.apply_compression(summary);
            return AgentAction::Continue;
        }

        let resp: LLMResponse = match serde_json::from_str(response_json) {
            Ok(r)  => r,
            Err(e) => return AgentAction::Error(format!("failed to parse LLM response: {}", e)),
        };

        let choice = match resp.choices.into_iter().next() {
            Some(c) => c,
            None    => return AgentAction::Error("LLM response has no choices".into()),
        };

        let finish_reason = choice.finish_reason.as_deref().unwrap_or("stop");

        match finish_reason {
            "tool_calls" => {
                let raw_calls = match choice.message.tool_calls {
                    Some(tc) if !tc.is_empty() => tc,
                    _ => return AgentAction::Error(
                        "finish_reason=tool_calls but no tool_calls in message".into(),
                    ),
                };

                // Intercept `ask_questions` before any tools are executed.
                // The model uses this tool to signal it wants clarification
                // rather than text-heuristic detection.
                if let Some(aq) = raw_calls.iter().find(|tc| tc.function.name == "ask_questions") {
                    let args  = aq.function.arguments.clone();
                    let aq_id = aq.id.clone();

                    // Record the assistant's tool-call in history so that when
                    // phase 2 starts the model can see what it asked.
                    let tool_calls: Vec<ToolCall> = raw_calls.iter().map(|tc| ToolCall {
                        id:        tc.id.clone(),
                        name:      tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    }).collect();
                    self.messages.push(Message {
                        role:         Role::Assistant,
                        content:      choice.message.content,
                        tool_calls,
                        tool_call_id: None,
                    });

                    // Anthropic (and OpenAI) require every tool_use block to be
                    // immediately followed by a tool_result.  Add a synthetic
                    // result so phase 2 doesn't get a 400 "tool_use ids were
                    // found without tool_result blocks" error.
                    self.messages.push(Message::tool_result(aq_id, "Questions received."));

                    return AgentAction::AskQuestions(args);
                }

                // Append the assistant message (with tool_calls) to history.
                let tool_calls: Vec<ToolCall> = raw_calls.iter().map(|tc| ToolCall {
                    id:        tc.id.clone(),
                    name:      tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                }).collect();

                self.messages.push(Message {
                    role:         Role::Assistant,
                    content:      choice.message.content,
                    tool_calls:   tool_calls.clone(),
                    tool_call_id: None,
                });

                // Execute each tool call and append results.
                for tc in &tool_calls {
                    let result = self.tools_manager.call(&tc.name, &tc.arguments);
                    self.messages.push(Message::tool_result(tc.id.clone(), result));
                }

                AgentAction::Continue
            }

            "stop" | "length" => {
                let truncated = finish_reason == "length";
                match choice.message.content {
                    Some(text) if !text.trim().is_empty() => {
                        if truncated {
                            eprintln!(
                                "  [architect] warning: output was truncated (finish_reason=length). \
                                 Increase OPENAI_ARCHITECT_MAX_TOKENS to get a complete document.",
                            );
                        }
                        self.messages.push(Message {
                            role:         Role::Assistant,
                            content:      Some(text.clone()),
                            tool_calls:   vec![],
                            tool_call_id: None,
                        });
                        AgentAction::AssistantMessage(text)
                    }
                    _ => AgentAction::Stop,
                }
            }

            other => AgentAction::Error(format!("unexpected finish_reason: {}", other)),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::ToolsManager;

    fn make_agent(model: &str, threshold_override: Option<usize>) -> LLMAgent {
        let mut agent = LLMAgent::new(
            vec![Message::system("system prompt")],
            ToolsManager::new(),
            model,
        );
        if let Some(t) = threshold_override {
            agent.threshold_chars = t;
        }
        agent
    }

    fn push_tool_exchange(agent: &mut LLMAgent, content: &str) {
        // assistant calls a tool
        agent.messages.push(Message {
            role:         Role::Assistant,
            content:      None,
            tool_calls:   vec![ToolCall {
                id:        "tc1".into(),
                name:      "read_file".into(),
                arguments: "{}".into(),
            }],
            tool_call_id: None,
        });
        // tool result with the given content
        agent.messages.push(Message::tool_result("tc1", content));
    }

    // ── context_window_for_model ──────────────────────────────────────────────

    #[test]
    fn model_lookup_claude_4_6() {
        assert_eq!(context_window_for_model("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(context_window_for_model("claude-opus-4-6"),   1_000_000);
        assert_eq!(context_window_for_model("Claude 4.6 Sonnet"), 1_000_000);
    }

    #[test]
    fn model_lookup_claude_4_5() {
        assert_eq!(context_window_for_model("claude-sonnet-4-5-20251001"), 1_000_000);
        assert_eq!(context_window_for_model("claude-haiku-4-5"),           1_000_000);
    }

    #[test]
    fn model_lookup_claude_old() {
        assert_eq!(context_window_for_model("claude-3-5-sonnet"),  200_000);
        assert_eq!(context_window_for_model("claude-3-opus"),       200_000);
        assert_eq!(context_window_for_model("claude-3.7-sonnet"),   200_000);
    }

    #[test]
    fn model_lookup_gpt_4o() {
        assert_eq!(context_window_for_model("gpt-4o"),      128_000);
        assert_eq!(context_window_for_model("gpt-4o-mini"), 128_000);
        assert_eq!(context_window_for_model("GPT-4o"),      128_000);
    }

    #[test]
    fn model_lookup_gpt_4_1() {
        assert_eq!(context_window_for_model("gpt-4.1"),      1_047_576);
        assert_eq!(context_window_for_model("gpt-4.1-mini"), 1_047_576);
    }

    #[test]
    fn model_lookup_gpt_5() {
        assert_eq!(context_window_for_model("gpt-5"),      400_000);
        assert_eq!(context_window_for_model("gpt-5-mini"), 400_000);
    }

    #[test]
    fn model_lookup_unknown_defaults_to_128k() {
        assert_eq!(context_window_for_model(""),               128_000);
        assert_eq!(context_window_for_model("some-new-model"), 128_000);
    }

    #[test]
    fn model_lookup_deepseek() {
        assert_eq!(context_window_for_model("deepseek-chat"), 64_000);
        assert_eq!(context_window_for_model("deepseek-r1"),   64_000);
    }

    // ── should_compress ───────────────────────────────────────────────────────

    #[test]
    fn no_compression_below_threshold() {
        let mut agent = make_agent("gpt-4o", Some(1_000));
        // Only 3 messages total — not enough (need >= 5).
        agent.messages.push(Message::user("hi"));
        agent.messages.push(Message::tool_result("x", "small"));
        assert!(!agent.should_compress());
    }

    #[test]
    fn compression_triggered_above_threshold() {
        let mut agent = make_agent("gpt-4o", Some(100));
        // Add enough messages and chars to exceed the 100-char threshold.
        let big = "x".repeat(30);
        for _ in 0..5 {
            push_tool_exchange(&mut agent, &big);
        }
        assert!(agent.should_compress());
    }

    // ── start_compression boundary ────────────────────────────────────────────

    #[test]
    fn compression_boundary_within_70_percent() {
        let mut agent = make_agent("gpt-4o", Some(100));
        // 10 tool exchanges, each with ~20 chars of content.
        for i in 0..10 {
            push_tool_exchange(&mut agent, &format!("result-{:015}", i));
        }
        let total_before = agent.messages.len();
        agent.start_compression();

        assert!(agent.compressing);
        // Boundary must be within the compressible range (not touching last 2).
        let safe_end = total_before - 2;
        assert!(agent.compression_end > 1);
        assert!(agent.compression_end <= safe_end);
    }

    // ── full compression round-trip ───────────────────────────────────────────

    fn compression_response_json(summary: &str) -> String {
        serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": summary },
                "finish_reason": "stop"
            }]
        })
        .to_string()
    }

    #[test]
    fn compression_round_trip_reduces_messages() {
        let mut agent = make_agent("gpt-4o", Some(100));
        let big = "x".repeat(30);
        for _ in 0..6 {
            push_tool_exchange(&mut agent, &big);
        }
        let count_before = agent.messages.len();

        // get_request should trigger compression.
        let req = agent.get_request();
        assert!(agent.compressing);

        // The compression request should NOT contain the normal tools.
        let parsed: serde_json::Value = serde_json::from_str(&req).unwrap();
        assert!(parsed["tools"].is_null() || parsed["tools"].as_array().map_or(true, |a| a.is_empty()));

        // Feed back a summary.
        let action = agent.process_response(&compression_response_json("SUMMARY OF HISTORY"));
        assert!(matches!(action, AgentAction::Continue));
        assert!(!agent.compressing);

        // Message count must have shrunk.
        assert!(agent.messages.len() < count_before);

        // The summary message must be present at index 1.
        assert!(agent.messages[1].content.as_deref().unwrap_or("").contains("SUMMARY OF HISTORY"));
        // System prompt must still be at index 0.
        assert_eq!(agent.messages[0].role, Role::System);
    }

    #[test]
    fn second_get_request_after_compression_is_normal() {
        let mut agent = make_agent("gpt-4o", Some(100));
        let big = "x".repeat(30);
        for _ in 0..6 {
            push_tool_exchange(&mut agent, &big);
        }

        // Trigger and apply compression.
        agent.get_request();
        agent.process_response(&compression_response_json("summary"));

        // Raise threshold so we don't compress again immediately.
        agent.threshold_chars = 1_000_000;

        // Next get_request should be a normal agent request with tools.
        let req = agent.get_request();
        let parsed: serde_json::Value = serde_json::from_str(&req).unwrap();
        // Normal requests have a "messages" array.
        assert!(parsed["messages"].is_array());
    }
}
