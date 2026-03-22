use std::rc::Rc;
use std::sync::Arc;

use serde_json::json;

use crate::agent::graph_tools::register_graph_tools;
use crate::agent::llm_agent::{AgentAction, LLMAgent};
use crate::agent::memory::Message;
use crate::agent::tools::{ToolDefinition, ToolsManager};
use crate::filesystem::FileSystem;
use crate::graph::DependencyGraph;

// ─── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are a senior product manager specializing in feature specification.

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.

You work in two phases:

── Phase 1: Exploration ──────────────────────────────────────────────────────
Explore the codebase to understand how the requested feature fits the project:
1. Call `read_architecture` first if it exists.
2. Use graph tools to find relevant modules, entry points, data models, and dependencies.
3. Call multiple independent tools in a single response.

After exploration, if you need clarification from the user, respond with ONLY this JSON
(no markdown, no preamble, no other text whatsoever):
{"questions":[{"id":"q1","text":"...","type":"open"},{"id":"q2","text":"...","type":"choice","choices":["A","B"]}]}

Rules for questions:
- Maximum 6 questions total.
- type "open"   → free-text answer.
- type "choice" → user picks exactly one of the provided choices (at least 2 choices required).
- Only ask questions that would meaningfully change the specification.
- If the feature is clear enough without clarification, skip to Phase 2 directly.

── Phase 2: Specification ────────────────────────────────────────────────────
Write a complete feature specification in Markdown written for a PRODUCT audience,
not a technical one. Avoid file names, class names, function names, and implementation details.
Focus on WHAT the feature does and WHY, not HOW it is built.

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
        graph:   DependencyGraph,
        root:    impl Into<String>,
        feature: impl Into<String>,
        fs:      Box<dyn FileSystem>,
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

        // Register shared graph exploration tools.
        register_graph_tools(&mut tools, Arc::new(graph), fs_rc);

        let user_msg = format!(
            "Project at `{root}` ({file_count} source files, {node_count} nodes).\n\n\
             Feature request: {feature}\n\n\
             Explore the codebase to understand how this feature fits the project, \
             then either ask clarification questions (as JSON) or generate the \
             feature specification directly.",
        );

        Self {
            agent: LLMAgent {
                messages:      vec![Message::system(SYSTEM_PROMPT), Message::user(user_msg)],
                tools_manager: tools,
            },
            root,
        }
    }

    pub fn get_request(&self) -> String {
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
