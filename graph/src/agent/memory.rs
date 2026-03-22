use serde::{Deserialize, Serialize};

// ─── Role ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

// ─── ToolCall ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id:        String,
    pub name:      String,
    /// Raw JSON string of arguments (as returned by the LLM).
    pub arguments: String,
}

// ─── Message ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Populated for `Role::Assistant` messages that carry tool invocations.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,

    /// Populated for `Role::Tool` result messages (links back to the call id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role:         Role::System,
            content:      Some(content.into()),
            tool_calls:   vec![],
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role:         Role::User,
            content:      Some(content.into()),
            tool_calls:   vec![],
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role:         Role::Tool,
            content:      Some(content.into()),
            tool_calls:   vec![],
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}
