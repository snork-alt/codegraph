use std::rc::Rc;
use std::sync::Arc;

use crate::agent::graph_tools::register_graph_tools;
use crate::agent::llm_agent::{AgentAction, LLMAgent};
use crate::agent::memory::Message;
use crate::agent::tools::ToolsManager;
use crate::filesystem::FileSystem;
use crate::graph::DependencyGraph;

// ─── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are a software architect assistant. Your goal is to thoroughly explore a codebase's dependency graph and produce a comprehensive architecture document in Markdown format.

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.
Set it to a concise sentence explaining why you are calling that tool at this moment.
Example: "Listing all files to get an overview of the project structure."
Example: "Checking what depends on OrderService to understand its coupling."
If you omit `__actionDetails__` from any tool call, your response is invalid.

Pagination:
All list-returning graph tools return a paginated envelope:
  { "items": [...], "total": N, "offset": O, "returned": K, "has_more": true/false }
Results are capped at 50 per call by default. When `has_more` is true, call the same tool
again with `"offset": O + K` to retrieve the next page. Keep paginating until you have the
data you need or `has_more` is false. You may also use `"limit"` to request fewer items.

Exploration strategy:
1. Call `list_files` first to get an overview of the project structure.
2. Call `find_nodes_by_kind` with kinds like "Class", "Interface", "Trait", "Enum" to discover all major types.
3. Use `get_file_summary` on key files to understand their contents.
4. Use `get_node_details` on important nodes to understand their roles.
5. Use `get_dependencies` and `get_dependents` to map relationships between components.
6. Use `search_nodes` to find specific named components when needed.
7. Repeat as needed until you have a complete picture.

IMPORTANT: You can — and should — call multiple tools in a single response whenever the calls are
independent of each other. For example, call `get_file_summary` on several files at once rather
than one per turn. This dramatically reduces the number of round-trips needed.

SOURCE CODE READING POLICY:
Use `get_file_source` sparingly — only when the graph metadata (node names, kinds, dependencies,
descriptions) is genuinely insufficient to understand a component's purpose or behaviour.
Good reasons to read source: understanding a non-obvious algorithm, resolving an ambiguous
architecture boundary, or confirming how two components interact at the implementation level.
Bad reasons: reading every file by default, satisfying general curiosity, or re-confirming
something already clear from the graph.
Prefer `get_node_details` and `get_dependencies` over reading source whenever possible.

Do not stop exploring until you have covered all major components and their relationships.

When you are confident you have a thorough understanding, respond with a single Markdown document that includes:
- **Project overview**: what the project does, the main languages/technologies used
- **Directory and module structure**: how files are organized
- **Key components**: each major class/interface/module with its responsibility
- **Dependency map**: how components relate to each other (imports, inheritance, calls)
- **Architecture patterns**: patterns you observed (layering, MVC, ports-and-adapters, etc.)
- **Mermaid diagrams**: at least one Mermaid diagram showing the component relationships

Use Mermaid code blocks for all diagrams (e.g. ```mermaid\ngraph TD\n...```).
Prefer `graph TD` for dependency/component diagrams and `classDiagram` for type hierarchies.
Do not use SVG.
"#;

// ─── SoftwareArchitectAgent ───────────────────────────────────────────────────

pub struct SoftwareArchitectAgent {
    agent: LLMAgent,
    root:  String,
}

impl SoftwareArchitectAgent {
    pub fn new(graph: DependencyGraph, root: impl Into<String>, fs: Box<dyn FileSystem>, model_name: &str) -> Self {
        let root       = root.into();
        let file_count = graph.by_file.len();
        let node_count = graph.nodes.len();
        let edge_count = graph.edges.len();

        let mut tools = ToolsManager::new();
        register_graph_tools(&mut tools, Arc::new(graph), Rc::from(fs));

        let user_msg = format!(
            "Explore the dependency graph for the project at `{root}`. \
             The graph contains {file_count} source files, {node_count} nodes, \
             and {edge_count} edges. \
             Use the tools to systematically explore the codebase, then produce \
             the full architecture document in Markdown (with Mermaid diagrams).",
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

    pub fn architecture_path(&self) -> Option<String> {
        if self.root.is_empty() {
            None
        } else {
            Some(format!("{}/.codegraph/architecture.md", self.root))
        }
    }
}
