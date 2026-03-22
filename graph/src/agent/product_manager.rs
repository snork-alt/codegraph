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

const SYSTEM_PROMPT: &str = r#"You are a senior product manager assistant. Your goal is to analyse a software project and produce a comprehensive product specification document in Markdown format.

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.
Set it to a concise sentence explaining why you are calling that tool at this moment.
If you omit `__actionDetails__` from any tool call, your response is invalid.

Exploration strategy — follow these steps in order:
1. Call `read_architecture` FIRST. The architecture document is your primary source of truth.
2. Use the graph tools (`list_files`, `find_nodes_by_kind`, `get_file_summary`, `get_node_details`,
   `get_dependencies`, `get_dependents`, `search_nodes`) to deepen your understanding of specific
   features, user-facing entry points, and data flows.
3. Use `get_file_source` only when you need to understand a specific business rule or user flow
   that cannot be inferred from names, descriptions, and the architecture document.

You can — and should — call multiple tools in a single response when the calls are independent.

SOURCE CODE READING POLICY:
Use `get_file_source` sparingly — only when the graph metadata and architecture document are
insufficient to understand a user-facing behaviour or business rule.

When you have a thorough understanding, respond with a single Markdown product specification
document that includes:
- **Product overview**: what the product does, who it is for, and the core value proposition
- **Features**: a structured list of features with descriptions from a user perspective
- **User flows**: step-by-step descriptions of the main user journeys
- **Data model**: key entities and their relationships, described in business terms
- **Integration points**: external systems, APIs, or services the product depends on
- **Mermaid diagrams**: at least one user-flow diagram and one feature/component map

Use Mermaid code blocks for all diagrams.
Prefer `flowchart TD` or `sequenceDiagram` for user flows and `graph TD` for feature maps.
Do not use SVG.
Write for a non-technical audience where possible — avoid implementation jargon.
"#;

// ─── ProductManagerAgent ──────────────────────────────────────────────────────

pub struct ProductManagerAgent {
    agent: LLMAgent,
    root:  String,
}

impl ProductManagerAgent {
    /// Create a new agent that will analyse `graph` and the existing
    /// `architecture.md` to produce `specs.md`.
    pub fn new(graph: DependencyGraph, root: impl Into<String>, fs: Box<dyn FileSystem>) -> Self {
        let root       = root.into();
        let file_count = graph.by_file.len();
        let node_count = graph.nodes.len();

        let fs_rc: Rc<dyn FileSystem> = Rc::from(fs);

        let mut tools = ToolsManager::new();

        // ── read_architecture (PM-specific tool) ─────────────────────────────
        {
            let fs  = Rc::clone(&fs_rc);
            let r   = root.clone();
            tools.register(
                ToolDefinition {
                    name:        "read_architecture".into(),
                    description: "Read the architecture document produced by the SoftwareArchitectAgent \
                                  (`architecture.md`). Call this FIRST before using any other tool.".into(),
                    parameters:  vec![],
                },
                move |_args| {
                    let path = format!("{}/.codegraph/architecture.md", r);
                    match fs.read(&path) {
                        Some(content) => serde_json::to_string(&json!({
                            "path":    path,
                            "content": content,
                        })).unwrap_or_default(),
                        None => serde_json::to_string(&json!({
                            "error": format!(
                                "architecture.md not found at {}. \
                                 Run 'codegraph architect' first to generate it.",
                                path,
                            ),
                        })).unwrap_or_default(),
                    }
                },
            );
        }

        // Register all shared graph-exploration tools.
        register_graph_tools(&mut tools, Arc::new(graph), fs_rc);

        let user_msg = format!(
            "Analyse the project at `{root}` ({file_count} source files, {node_count} nodes). \
             Start by calling `read_architecture` to load the architecture document, then use \
             the graph tools to deepen your understanding of features and user flows. \
             Produce a complete product specification document in Markdown with Mermaid diagrams.",
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

    pub fn specs_path(&self) -> Option<String> {
        if self.root.is_empty() {
            None
        } else {
            Some(format!("{}/.codegraph/specs.md", self.root))
        }
    }
}
