use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ─── Parameter types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    String,
    Number,
    Boolean,
    Array,
}

impl ParamKind {
    fn as_json_type(&self) -> &'static str {
        match self {
            ParamKind::String  => "string",
            ParamKind::Number  => "number",
            ParamKind::Boolean => "boolean",
            ParamKind::Array   => "array",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    pub name:        String,
    pub kind:        ParamKind,
    pub description: String,
    pub required:    bool,
}

// ─── ToolDefinition ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name:        String,
    pub description: String,
    pub parameters:  Vec<ToolParameter>,
}

impl ToolDefinition {
    /// Render this definition as an OpenAI-compatible function tool JSON object.
    pub fn to_openai_spec(&self) -> Value {
        let mut properties: serde_json::Map<String, Value> = serde_json::Map::new();
        let mut required_keys: Vec<Value> = Vec::new();

        for p in &self.parameters {
            properties.insert(p.name.clone(), json!({
                "type":        p.kind.as_json_type(),
                "description": p.description,
            }));
            if p.required {
                required_keys.push(Value::String(p.name.clone()));
            }
        }

        json!({
            "type": "function",
            "function": {
                "name":        self.name,
                "description": self.description,
                "strict": true,
                "parameters": {
                    "type":                 "object",
                    "properties":           properties,
                    "required":             required_keys,
                    "additionalProperties": false,
                }
            }
        })
    }
}

// ─── ToolsManager ─────────────────────────────────────────────────────────────

/// Registry of tool definitions and their handlers.
///
/// Every registered tool automatically receives an `__actionDetails__`
/// string parameter (injected into its spec) that the LLM can use to
/// describe *why* it is calling the tool.
pub struct ToolsManager {
    definitions: Vec<ToolDefinition>,
    handlers:    HashMap<String, Box<dyn Fn(&str) -> String>>,
}

impl ToolsManager {
    pub fn new() -> Self {
        Self {
            definitions: Vec::new(),
            handlers:    HashMap::new(),
        }
    }

    /// Register a tool definition and its handler function.
    ///
    /// An `__actionDetails__` parameter is automatically injected into
    /// `def.parameters` so the LLM can narrate its reasoning.
    pub fn register(
        &mut self,
        mut def: ToolDefinition,
        handler: impl Fn(&str) -> String + 'static,
    ) {
        def.parameters.insert(0, ToolParameter {
            name:        "__actionDetails__".into(),
            kind:        ParamKind::String,
            description: "Required. A concise sentence explaining why this tool is being called right now.".into(),
            required:    true,
        });
        self.handlers.insert(def.name.clone(), Box::new(handler));
        self.definitions.push(def);
    }

    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    /// Invoke the handler for `name` with the raw JSON argument string.
    ///
    /// Prints `[tool] <name>: <__actionDetails__>` to stderr before calling
    /// the handler so progress is visible in the terminal.
    ///
    /// Returns a JSON error string if the tool is not found.
    pub fn call(&self, name: &str, args_json: &str) -> String {
        // Extract __actionDetails__ for logging.
        let details = serde_json::from_str::<serde_json::Value>(args_json)
            .ok()
            .and_then(|v| v["__actionDetails__"].as_str().map(str::to_owned))
            .unwrap_or_default();

        if details.is_empty() {
            eprintln!("  [tool] {name}");
        } else {
            eprintln!("  [tool] {name}: {details}");
        }

        match self.handlers.get(name) {
            Some(handler) => handler(args_json),
            None          => format!(r#"{{"error":"unknown tool: {}"}}"#, name),
        }
    }

    /// Serialize all registered tools as an OpenAI `tools` array.
    pub fn to_openai_tools(&self) -> Value {
        Value::Array(
            self.definitions.iter().map(|d| d.to_openai_spec()).collect(),
        )
    }
}
