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

const SYSTEM_PROMPT: &str = r#"You are a senior product manager specializing in feature specification.

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.

You work in two phases:

Pagination:
All list-returning graph tools return a paginated envelope:
  { "items": [...], "total": N, "offset": O, "returned": K, "has_more": true/false }
Results are capped at 50 per call by default. When `has_more` is true, call the same tool
again with `"offset": O + K` to retrieve the next page. Keep paginating until you have the
data you need or `has_more` is false. You may also use `"limit"` to request fewer items.

── Phase 1: Exploration ──────────────────────────────────────────────────────
Explore the codebase to understand how the requested feature fits the project:
1. Call `read_architecture` first if it exists.
2. Use graph tools to find relevant modules, entry points, data models, and dependencies.
3. Call multiple independent tools in a single response.

After exploration, if anything is unclear or ambiguous, call `ask_questions` with up to 6
clarification questions before writing the specification. Skip this step if the feature
request is already clear enough to write a good specification without further input.
- type "open"   → free-text answer.
- type "select" → user picks exactly one of the provided choices.
- type "multi"  → user picks one or more of the provided choices.
- Only ask questions that will meaningfully change the specification: scope, target users, priorities, edge cases.

── Phase 2: Specification ────────────────────────────────────────────────────
Write a complete feature specification in Markdown written for a PRODUCT audience,
not a technical one. Avoid file names, class names, function names, and implementation details.
Focus on WHAT the feature does and WHY, not HOW it is built.

CRITICAL OUTPUT RULE: The output MUST start with the title line — absolutely NO introductory
text, preamble, summary of exploration, or sentences like "Based on my analysis…" before it.
The very first character of your response must be `#`.

The VERY FIRST line must be the title in this exact format:
# <Feature Title>
(Keep the title concise — 3 to 6 words — it becomes the directory name.)

Then include these sections:

## Overview
What the feature does, who it is for, and the value it delivers. 2-4 sentences.

## User Stories
"As a <persona> I want <goal> so that <benefit>."
Each story on its own bullet. Focus on user goals, not system behaviour.

## Acceptance Criteria
A bullet list of observable, testable outcomes from the user's perspective.
Write each criterion as: "Given … when … then …" or a plain statement the user can verify.
No mention of code, APIs, or internal modules.

## User Flow
A step-by-step description of how a user interacts with the feature from start to finish.
Include a Mermaid `sequenceDiagram` or `flowchart TD` to illustrate the flow.

## Out of Scope
Explicitly list what this feature does NOT include in this iteration.

## Open Questions
Any unresolved product decisions. Omit this section if there are none.

Do not include sections about files to modify, classes, functions, dependencies, or architecture.
Do not use SVG.
"#;

// ─── NewFeatureProductManagerAgent ───────────────────────────────────────────

pub struct NewFeatureProductManagerAgent {
    agent: LLMAgent,
    root:  String,
}

impl NewFeatureProductManagerAgent {
    pub fn new(
        graph:      DependencyGraph,
        root:       impl Into<String>,
        feature:    impl Into<String>,
        fs:         Box<dyn FileSystem>,
        model_name: &str,
    ) -> Self {
        let root       = root.into();
        let feature    = feature.into();
        let file_count = graph.by_file.len();
        let node_count = graph.nodes.len();

        let fs_rc: Rc<dyn FileSystem> = Rc::from(fs);
        let mut tools = ToolsManager::new();

        // ── read_architecture ────────────────────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            let r  = root.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_architecture".into(),
                    description: "Read the architecture document (`architecture.md`). \
                                  Call this first to understand the overall system design.".into(),
                    parameters:  vec![],
                },
                move |_args| {
                    let path = format!("{}/.codegraph/architecture.md", r);
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path": path, "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": format!(
                                "architecture.md not found at {}. \
                                 Run 'codegraph architect' first.",
                                path,
                            ),
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // ── read_specs ───────────────────────────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            let r  = root.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_specs".into(),
                    description: "Read the product specification document (`specs.md`). \
                                  Useful to understand existing product context.".into(),
                    parameters:  vec![],
                },
                move |_args| {
                    let path = format!("{}/.codegraph/specs.md", r);
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path": path, "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": "specs.md not found. Run 'codegraph product-manager' first.",
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // ── ask_questions ─────────────────────────────────────────────────────
        tools.register(
            ToolDefinition {
                name:        "ask_questions".into(),
                description: "Ask the user clarification questions before writing the feature \
                              specification. Call this ONCE after completing exploration, with \
                              3–6 questions.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "questions".into(),
                        kind:        ParamKind::Schema(serde_json::json!({
                            "type": "array",
                            "description": "3 to 6 clarification questions.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id":      { "type": "string", "description": "Unique ID, e.g. q1, q2." },
                                    "text":    { "type": "string", "description": "The question text." },
                                    "type":    { "type": "string", "enum": ["open", "select", "multi"],
                                                 "description": "open = free-text; select = pick one; multi = pick one or more." },
                                    "choices": { "type": "array", "items": { "type": "string" },
                                                 "description": "Required when type=choice. Provide at least 2 options." }
                                },
                                "required": ["id", "text", "type", "choices"],
                                "additionalProperties": false
                            }
                        })),
                        description: "3 to 6 clarification questions.".into(),
                        required:    true,
                    },
                ],
            },
            |_args| "Questions received.".to_owned(),
        );

        // Register shared graph exploration tools.
        register_graph_tools(&mut tools, Arc::new(graph), fs_rc);

        let user_msg = format!(
            "Project at `{root}` ({file_count} source files, {node_count} nodes).\n\n\
             Feature request: {feature}\n\n\
             Explore the codebase to understand how this feature fits the project, \
             then write the specification — or call `ask_questions` first if clarification is needed.",
        );

        Self {
            agent: LLMAgent::new(
                vec![Message::system(SYSTEM_PROMPT), Message::user(user_msg)],
                tools,
                model_name,
            ),
            root,
        }
    }

    pub fn get_request(&mut self) -> String {
        self.agent.get_request()
    }

    pub fn process_response(&mut self, response_json: &str) -> AgentAction {
        self.agent.process_response(response_json)
    }

    /// Inject the user's answers to clarification questions as a new user
    /// message so the agent can proceed to specification generation.
    pub fn submit_answers(&mut self, answers_json: &str) {
        let msg = format!(
            "Here are my answers to the clarification questions:\n\n{}\n\n\
             Please now generate the complete feature specification.",
            answers_json,
        );
        self.agent.messages.push(Message::user(msg));
    }

    pub fn root(&self) -> &str {
        &self.root
    }
}
