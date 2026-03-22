use std::rc::Rc;
use std::sync::Arc;

use serde_json::json;

use crate::agent::graph_tools::register_graph_tools;
use crate::agent::llm_agent::{AgentAction, LLMAgent};
use crate::agent::memory::Message;
use crate::agent::tools::{ToolDefinition, ToolParameter, ParamKind, ToolsManager};
use crate::filesystem::FileSystem;
use crate::graph::DependencyGraph;

// ─── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are a senior software architect specialising in feature implementation planning.
Your goal is to read a feature specification and produce a detailed, actionable technical implementation plan.

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.

── Phase 1: Exploration ──────────────────────────────────────────────────────
1. Call `read_feature_spec` FIRST to read the feature specification.
2. Call `read_architecture` to understand the overall system design.
3. Use graph tools to explore the codebase in depth:
   - Identify files, modules, and components that will need to change.
   - Find existing patterns to follow (similar features, analogous code paths).
   - Understand data models, interfaces, and cross-cutting concerns.
   - Use `get_file_source` when you need to understand a specific implementation.
4. Call multiple independent tools in a single response.

After exploration, if you need clarification from the developer (max 6 questions):
Respond with ONLY this JSON (no other text):
{"questions":[{"id":"q1","text":"...","type":"open"},{"id":"q2","text":"...","type":"choice","choices":["A","B"]}]}

type "open"   → free-text answer.
type "choice" → developer picks one of the provided choices.
Only ask questions that would meaningfully change the implementation plan.

── Phase 2: Implementation Plan ──────────────────────────────────────────────
Write a detailed technical implementation plan in Markdown.
CRITICAL OUTPUT RULE: NO introductory text, preamble, or sentences like "Based on my
exploration…" before the title. The very first character of your response must be `#`.
The VERY FIRST line must be:
# <Feature Title> — Implementation Plan

Then include:

## Summary
One paragraph describing the implementation approach and key decisions.

## Architecture Changes
How the system architecture changes: new components, modified interfaces, data flow changes.
Include a Mermaid diagram (`flowchart TD` or `graph TD`) showing affected components.

## Implementation Steps
Numbered, ordered steps a developer should follow.
Each step should be self-contained and completable in isolation where possible.
Reference specific files, functions, or modules by name.

## Files to Create or Modify
A table or bullet list:
- **`path/to/file.rs`** (create) — what it contains
- **`path/to/other.rs`** (modify) — what changes

## Testing Strategy
- Unit tests: what to test and where
- Integration tests: what scenarios to cover
- Edge cases to handle

## Dependencies
New crates, libraries, or tools required. Include version if known.

## Risks and Mitigations
Potential issues and how to address them.

Use Mermaid code blocks for diagrams. Do not use SVG.
Be specific — name actual files, structs, traits, and functions from the codebase.
"#;

// ─── NewFeatureArchitectAgent ─────────────────────────────────────────────────

pub struct NewFeatureArchitectAgent {
    agent:        LLMAgent,
    feature_path: String,
}

impl NewFeatureArchitectAgent {
    /// Create a new agent that will plan the implementation of the feature
    /// whose `specs.md` lives at `<feature_path>/specs.md`.
    pub fn new(
        graph:        DependencyGraph,
        root:         impl Into<String>,
        feature_path: impl Into<String>,
        fs:           Box<dyn FileSystem>,
    ) -> Self {
        let root         = root.into();
        let feature_path = feature_path.into();
        let file_count   = graph.by_file.len();
        let node_count   = graph.nodes.len();

        let fs_rc: Rc<dyn FileSystem> = Rc::from(fs);
        let mut tools = ToolsManager::new();

        // ── read_feature_spec ─────────────────────────────────────────────────
        {
            let fs  = Rc::clone(&fs_rc);
            let fp  = feature_path.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_feature_spec".into(),
                    description: "Read the product specification (`specs.md`) for the feature \
                                  being planned. Call this FIRST.".into(),
                    parameters:  vec![],
                },
                move |_args| {
                    let path = format!("{}/specs.md", fp);
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path": path, "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": format!("specs.md not found at {}.", path),
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // ── read_architecture ────────────────────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            let r  = root.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_architecture".into(),
                    description: "Read the architecture document (`architecture.md`).".into(),
                    parameters:  vec![],
                },
                move |_args| {
                    let path = format!("{}/.codegraph/architecture.md", r);
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path": path, "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": "architecture.md not found.",
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // ── read_file (generic source reader) ────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            tools.register(
                ToolDefinition {
                    name:        "read_file".into(),
                    description: "Read the raw source of any file in the project by absolute path. \
                                  Use sparingly — prefer graph tools first.".into(),
                    parameters:  vec![
                        ToolParameter {
                            name:        "path".into(),
                            kind:        ParamKind::String,
                            description: "Absolute path to the file.".into(),
                            required:    true,
                        },
                    ],
                },
                move |args| {
                    let path = serde_json::from_str::<serde_json::Value>(args)
                        .ok()
                        .and_then(|v| v["path"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path": path, "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": format!("file not found: {}", path),
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // Register all shared graph exploration tools.
        register_graph_tools(&mut tools, Arc::new(graph), fs_rc);

        let user_msg = format!(
            "Project at `{root}` ({file_count} source files, {node_count} nodes).\n\
             Feature path: `{feature_path}`\n\n\
             Start by calling `read_feature_spec` to read the product specification, \
             then explore the codebase to understand what needs to change. \
             Produce a detailed technical implementation plan.",
        );

        Self {
            agent: LLMAgent {
                messages:      vec![Message::system(SYSTEM_PROMPT), Message::user(user_msg)],
                tools_manager: tools,
            },
            feature_path,
        }
    }

    pub fn get_request(&self) -> String {
        self.agent.get_request()
    }

    pub fn process_response(&mut self, response_json: &str) -> AgentAction {
        self.agent.process_response(response_json)
    }

    pub fn submit_answers(&mut self, answers_json: &str) {
        let msg = format!(
            "Here are my answers to the clarification questions:\n\n{}\n\n\
             Please now produce the complete technical implementation plan.",
            answers_json,
        );
        self.agent.messages.push(Message::user(msg));
    }

    pub fn feature_path(&self) -> &str {
        &self.feature_path
    }
}
