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

const SYSTEM_PROMPT: &str = r#"You are a senior software engineer tasked with producing a precise,
step-by-step implementation task list that will be executed by an AI coding agent (GitHub Copilot).

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.

Pagination:
All list-returning graph tools return a paginated envelope:
  { "items": [...], "total": N, "offset": O, "returned": K, "has_more": true/false }
Results are capped at 50 per call by default. When `has_more` is true, call the same tool
again with `"offset": O + K` to retrieve the next page. Keep paginating until you have the
data you need or `has_more` is false. You may also use `"limit"` to request fewer items.

── Phase 1: Deep Exploration ─────────────────────────────────────────────────
1. Call `read_feature_spec` FIRST to read the feature specification.
2. Call `read_feature_plan` to read the implementation plan.
3. Use ALL available graph tools to explore the codebase in depth:
   - Read every file that will need to change.
   - Understand the exact function signatures, struct fields, traits, and types involved.
   - Find existing patterns to replicate (similar features, analogous code paths).
   - Read test files to understand the testing style.
4. Use `get_file_source` liberally — understanding actual code is essential.
5. Call multiple independent tools in a single response.

── Phase 2: Tasks Document ───────────────────────────────────────────────────
Produce a detailed tasks.md.
CRITICAL OUTPUT RULE: NO introductory text, preamble, or sentences like "Based on my
exploration…" before the title. The very first character of your response must be `#`.
The VERY FIRST line must be:
# <Feature Title> — Tasks

Then include:

## Overview
One concise paragraph describing the implementation sequence and key decisions.

## Task 1: <short imperative title>
**Files:** `path/to/file` (create|modify)
**What:** Exact description of the change — function to add, field to insert, interface to extend.
**How:** Step-by-step instructions referencing actual existing code:
  - Exact function signatures to add or modify (with parameter types and return types)
  - Existing patterns to follow (reference specific file:function)
  - New types, structs, or interfaces to define

## Task 2: …

(continue for all tasks in dependency order)

## Task N: Verification
- Build command(s) to confirm no compilation errors
- Test commands and what passing looks like
- Manual verification steps if applicable

RULES:
- Every task must be atomic — completable in isolation by a coding agent.
- Never say "update the code" or "handle the error" without specifying exactly what and where.
- Reference real file paths, function names, and types from the codebase — not invented ones.
- Tasks must be ordered so that later tasks can depend on earlier ones.
- Include a task for any new tests that need to be written.
- Use Markdown code blocks for code snippets (with language tag).
"#;

// ─── NewFeatureSoftwareEngineerAgent ─────────────────────────────────────────

pub struct NewFeatureSoftwareEngineerAgent {
    agent:        LLMAgent,
    feature_path: String,
}

impl NewFeatureSoftwareEngineerAgent {
    /// Create a new agent that will produce `tasks.md` for the feature whose
    /// `specs.md` and `plan.md` live at `<feature_path>/`.
    pub fn new(
        graph:        DependencyGraph,
        root:         impl Into<String>,
        feature_path: impl Into<String>,
        fs:           Box<dyn FileSystem>,
        model_name:   &str,
    ) -> Self {
        let root         = root.into();
        let feature_path = feature_path.into();
        let file_count   = graph.by_file.len();
        let node_count   = graph.nodes.len();

        let fs_rc: Rc<dyn FileSystem> = Rc::from(fs);
        let mut tools = ToolsManager::new();

        // ── read_feature_spec ─────────────────────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            let fp = feature_path.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_feature_spec".into(),
                    description: "Read the product specification (`specs.md`) for the feature. \
                                  Call this FIRST.".into(),
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

        // ── read_feature_plan ─────────────────────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            let fp = feature_path.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_feature_plan".into(),
                    description: "Read the technical implementation plan (`plan.md`) for the feature. \
                                  Call this second, after `read_feature_spec`.".into(),
                    parameters:  vec![],
                },
                move |_args| {
                    let path = format!("{}/plan.md", fp);
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path": path, "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": format!("plan.md not found at {}.", path),
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // ── read_architecture ─────────────────────────────────────────────────
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

        // ── read_file (generic source reader) ─────────────────────────────────
        {
            let fs = Rc::clone(&fs_rc);
            tools.register(
                ToolDefinition {
                    name:        "read_file".into(),
                    description: "Read the raw source of any file in the project by absolute path. \
                                  Use this to understand existing code patterns before writing tasks.".into(),
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
             Start by calling `read_feature_spec` and `read_feature_plan` to understand what needs \
             to be built, then explore the codebase deeply to understand all the files that need \
             to change. Produce a detailed, atomic task list in tasks.md format.",
        );

        Self {
            agent: LLMAgent::new(
                vec![Message::system(SYSTEM_PROMPT), Message::user(user_msg)],
                tools,
                model_name,
            ),
            feature_path,
        }
    }

    pub fn get_request(&mut self) -> String {
        self.agent.get_request()
    }

    pub fn process_response(&mut self, response_json: &str) -> AgentAction {
        self.agent.process_response(response_json)
    }

    pub fn feature_path(&self) -> &str {
        &self.feature_path
    }
}
