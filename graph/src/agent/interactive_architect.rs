use std::rc::Rc;
use std::sync::Arc;

use crate::agent::graph_tools::register_graph_tools;
use crate::agent::llm_agent::{AgentAction, LLMAgent};
use crate::agent::memory::Message;
use crate::agent::tools::ToolsManager;
use crate::filesystem::FileSystem;
use crate::graph::DependencyGraph;

// ─── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are an expert software architect assistant.
Your goal is to answer architectural questions about a software project accurately and concisely.

CRITICAL RULE — you MUST include `__actionDetails__` on every tool call, no exceptions.
Set it to a concise sentence explaining why you are calling that tool at this moment.

Exploration strategy:
1. Use the graph tools to gather the evidence needed to answer the question.
2. Call multiple independent tools in a single response to explore efficiently.
3. Use `get_file_source` only when the graph metadata is insufficient to answer the question.
4. Stop exploring as soon as you have enough information — do not over-explore.

When you have enough information, respond with a clear and concise answer that:
- Directly addresses the question
- References specific files, modules, or nodes by name where relevant
- Includes a Mermaid diagram only when it genuinely clarifies the answer
- Avoids implementation jargon where possible

Use `flowchart TD` or `sequenceDiagram` for flows and `graph TD` for structure diagrams.
Do not use SVG.
"#;

// ─── InteractiveArchitectAgent ────────────────────────────────────────────────

pub struct InteractiveArchitectAgent {
    agent: LLMAgent,
}

impl InteractiveArchitectAgent {
    /// Create a new agent that will answer `question` about `graph`.
    pub fn new(
        graph:    DependencyGraph,
        root:     impl Into<String>,
        question: impl Into<String>,
        fs:       Box<dyn FileSystem>,
    ) -> Self {
        let root       = root.into();
        let question   = question.into();
        let file_count = graph.by_file.len();
        let node_count = graph.nodes.len();

        let fs_rc: Rc<dyn FileSystem> = Rc::from(fs);
        let mut tools = ToolsManager::new();
        register_graph_tools(&mut tools, Arc::new(graph), fs_rc);

        let user_msg = format!(
            "The project is at `{root}` ({file_count} source files, {node_count} nodes).\n\n\
             Question: {question}",
        );

        Self {
            agent: LLMAgent {
                messages:      vec![Message::system(SYSTEM_PROMPT), Message::user(user_msg)],
                tools_manager: tools,
            },
        }
    }

    pub fn get_request(&self) -> String {
        self.agent.get_request()
    }

    pub fn process_response(&mut self, response_json: &str) -> AgentAction {
        self.agent.process_response(response_json)
    }
}
