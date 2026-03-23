use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::memory::{Message, Role, ToolCall};
use crate::agent::tools::ToolsManager;

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
}

impl LLMAgent {
    /// Serialize the current conversation + tools into a JSON request that
    /// TypeScript should forward to the LLM.
    pub fn get_request(&self) -> String {
        let request = LLMRequest {
            messages: self.messages.iter().map(to_wire).collect(),
            tools:    self.tools_manager.to_openai_tools(),
            model:    None, // TypeScript injects this from env
        };
        serde_json::to_string(&request).unwrap_or_else(|e| {
            format!(r#"{{"error":"failed to serialize request: {}"}}"#, e)
        })
    }

    /// Process a raw OpenAI-compatible LLM response JSON string.
    ///
    /// - If the LLM returned tool calls: execute them, append all results to
    ///   `messages`, and return `AgentAction::Continue`.
    /// - If the LLM produced a final text answer: return
    ///   `AgentAction::AssistantMessage(text)`.
    /// - Otherwise: return `AgentAction::Stop` or `AgentAction::Error`.
    pub fn process_response(&mut self, response_json: &str) -> AgentAction {
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
