use std::rc::Rc;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::tools::{ParamKind, ToolDefinition, ToolParameter, ToolsManager};
use crate::explorer::GraphExplorer;
use crate::filesystem::FileSystem;
use crate::graph::{DependencyGraph, EdgeTarget, NodeId, NodeKind};

// ─── Node helpers ─────────────────────────────────────────────────────────────

pub fn node_summary(graph: &DependencyGraph, id: NodeId) -> Value {
    match graph.get_node(id) {
        None => json!({ "error": "node not found" }),
        Some(n) => json!({
            "id":             n.id,
            "kind":           format!("{:?}", n.kind),
            "name":           n.name,
            "qualified_name": n.qualified_name,
            "file":           n.file,
            "visibility":     format!("{:?}", n.visibility),
            "description":    n.description,
        }),
    }
}

pub fn node_details(graph: &DependencyGraph, id: NodeId) -> Value {
    match graph.get_node(id) {
        None => json!({ "error": "node not found" }),
        Some(n) => json!({
            "id":               n.id,
            "kind":             format!("{:?}", n.kind),
            "name":             n.name,
            "qualified_name":   n.qualified_name,
            "file":             n.file,
            "span": {
                "start_line": n.span.start_line,
                "end_line":   n.span.end_line,
            },
            "language":         format!("{:?}", n.language),
            "visibility":       format!("{:?}", n.visibility),
            "is_async":         n.is_async,
            "is_abstract":      n.is_abstract,
            "is_constructor":   n.is_constructor,
            "type_annotation":  n.type_annotation,
            "generic_params":   n.generic_params,
            "generic_bounds":   n.generic_bounds,
            "attributes":       n.attributes,
            "description":      n.description,
        }),
    }
}

pub fn parse_node_kind(s: &str) -> Option<NodeKind> {
    match s {
        "File"            => Some(NodeKind::File),
        "Package"         => Some(NodeKind::Package),
        "Class"           => Some(NodeKind::Class),
        "Interface"       => Some(NodeKind::Interface),
        "Trait"           => Some(NodeKind::Trait),
        "Enum"            => Some(NodeKind::Enum),
        "Annotation"      => Some(NodeKind::Annotation),
        "TypeAlias"       => Some(NodeKind::TypeAlias),
        "Function"        => Some(NodeKind::Function),
        "Method"          => Some(NodeKind::Method),
        "Field"           => Some(NodeKind::Field),
        "StaticField"     => Some(NodeKind::StaticField),
        "Constant"        => Some(NodeKind::Constant),
        "Variable"        => Some(NodeKind::Variable),
        "Parameter"       => Some(NodeKind::Parameter),
        "TypeParameter"   => Some(NodeKind::TypeParameter),
        "Closure"         => Some(NodeKind::Closure),
        "GlobalVariable"  => Some(NodeKind::GlobalVariable),
        "Import"          => Some(NodeKind::Import),
        "ExternalPackage" => Some(NodeKind::ExternalPackage),
        _                 => None,
    }
}

// ─── Shared graph tool registration ──────────────────────────────────────────
//
// Registers the 8 graph-exploration tools shared by all agents:
//   list_files, get_file_summary, get_node_details, get_dependencies,
//   get_dependents, find_nodes_by_kind, search_nodes, get_file_source

pub fn register_graph_tools(
    tools: &mut ToolsManager,
    graph: Arc<DependencyGraph>,
    fs:    Rc<dyn FileSystem>,
) {
    // ── list_files ───────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "list_files".into(),
                description: "List all source files in the graph with a count of how many nodes each file contains.".into(),
                parameters:  vec![],
            },
            move |_args| {
                let mut files: Vec<Value> = g.by_file.iter()
                    .map(|(file, ids)| json!({ "file": file, "node_count": ids.len() }))
                    .collect();
                files.sort_by(|a, b| {
                    a["file"].as_str().unwrap_or("").cmp(b["file"].as_str().unwrap_or(""))
                });
                serde_json::to_string(&files).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_file_summary ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_file_summary".into(),
                description: "Get a list of all nodes (classes, functions, fields, etc.) declared in a specific source file.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "file".into(),
                        kind:        ParamKind::String,
                        description: "The file path as it appears in the graph.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let file = match v["file"].as_str() {
                    Some(f) => f,
                    None    => return r#"{"error":"missing 'file' parameter"}"#.into(),
                };
                match g.by_file.get(file) {
                    None      => format!(r#"{{"error":"file not found: {}"}}"#, file),
                    Some(ids) => {
                        let nodes: Vec<Value> = ids.iter().map(|&id| node_summary(&g, id)).collect();
                        serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
                    }
                }
            },
        );
    }

    // ── get_node_details ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_node_details".into(),
                description: "Get full details of a node by its qualified name.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the node.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                match g.find_by_qualified(qname) {
                    None     => format!(r#"{{"error":"node not found: {}"}}"#, qname),
                    Some(id) => serde_json::to_string(&node_details(&g, id)).unwrap_or_default(),
                }
            },
        );
    }

    // ── get_dependencies ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_dependencies".into(),
                description: "Get the nodes that a given node depends on (outgoing edges: calls, extends, implements, imports, etc.).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the source node.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let edges: Vec<Value> = g.edges.iter()
                    .filter(|e| e.from == id)
                    .map(|e| {
                        let target = match &e.to {
                            EdgeTarget::Resolved(tid) => g.get_node(*tid)
                                .map(|n| n.qualified_name.clone())
                                .unwrap_or_else(|| format!("#{}", tid)),
                            EdgeTarget::External(s)   => format!("external:{}", s),
                            EdgeTarget::Unresolved(s) => format!("unresolved:{}", s),
                        };
                        json!({ "kind": format!("{:?}", e.kind), "target": target })
                    })
                    .collect();
                serde_json::to_string(&edges).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_dependents ───────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_dependents".into(),
                description: "Get the nodes that depend on a given node (incoming edges).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the target node.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let edges: Vec<Value> = g.edges.iter()
                    .filter(|e| matches!(&e.to, EdgeTarget::Resolved(tid) if *tid == id))
                    .map(|e| {
                        let source = g.get_node(e.from)
                            .map(|n| n.qualified_name.clone())
                            .unwrap_or_else(|| format!("#{}", e.from));
                        json!({ "kind": format!("{:?}", e.kind), "source": source })
                    })
                    .collect();
                serde_json::to_string(&edges).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── find_nodes_by_kind ───────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "find_nodes_by_kind".into(),
                description: "Find all nodes of a given kind. Valid kinds: File, Package, Class, Interface, Trait, Enum, Annotation, TypeAlias, Function, Method, Field, StaticField, Constant, Variable, Parameter, TypeParameter, Closure, GlobalVariable, Import, ExternalPackage.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "kind".into(),
                        kind:        ParamKind::String,
                        description: "The NodeKind to search for (case-sensitive, e.g. 'Class').".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "limit".into(),
                        kind:        ParamKind::Number,
                        description: "Maximum number of results to return (default: 50).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let kind_str = match v["kind"].as_str() {
                    Some(k) => k,
                    None    => return r#"{"error":"missing 'kind' parameter"}"#.into(),
                };
                let limit = v["limit"].as_u64().unwrap_or(50) as usize;
                let explorer = GraphExplorer::new(&g);
                match parse_node_kind(kind_str) {
                    None    => format!(r#"{{"error":"unknown node kind: {}"}}"#, kind_str),
                    Some(k) => {
                        let nodes: Vec<Value> = explorer.nodes_of_kind(k).into_iter()
                            .take(limit)
                            .map(|id| node_summary(&g, id))
                            .collect();
                        serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
                    }
                }
            },
        );
    }

    // ── search_nodes ─────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "search_nodes".into(),
                description: "Search for nodes whose name or qualified name contains the given query string (case-insensitive substring match).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "query".into(),
                        kind:        ParamKind::String,
                        description: "Substring to search for in node names.".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "limit".into(),
                        kind:        ParamKind::Number,
                        description: "Maximum number of results to return (default: 20).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let query = match v["query"].as_str() {
                    Some(q) => q.to_lowercase(),
                    None    => return r#"{"error":"missing 'query' parameter"}"#.into(),
                };
                let limit = v["limit"].as_u64().unwrap_or(20) as usize;
                let nodes: Vec<Value> = g.nodes.values()
                    .filter(|n| {
                        n.name.to_lowercase().contains(&query)
                            || n.qualified_name.to_lowercase().contains(&query)
                    })
                    .take(limit)
                    .map(|n| node_summary(&g, n.id))
                    .collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_graph_summary ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_graph_summary".into(),
                description: "Return total node/edge counts broken down by kind. Good first call to understand the scale and composition of the codebase.".into(),
                parameters:  vec![],
            },
            move |_args| {
                let explorer = GraphExplorer::new(&g);
                let s = explorer.summary();
                serde_json::to_string(&json!({
                    "total_nodes": s.total_nodes,
                    "total_edges": s.total_edges,
                    "node_counts": s.node_counts,
                    "edge_counts": s.edge_counts,
                })).unwrap_or_default()
            },
        );
    }

    // ── get_callers ───────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_callers".into(),
                description: "Get all methods/functions that call the given node (transitive upstream callers via Calls edges).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the callee node.".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "depth".into(),
                        kind:        ParamKind::Number,
                        description: "Max traversal depth (default: unlimited).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.upstream_callers(id, depth).into_iter()
                    .map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_callees ───────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_callees".into(),
                description: "Get all methods/functions transitively called by the given node (downstream calls via Calls edges).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the caller node.".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "depth".into(),
                        kind:        ParamKind::Number,
                        description: "Max traversal depth (default: unlimited).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.downstream_calls(id, depth).into_iter()
                    .map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_implementors ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_implementors".into(),
                description: "Get all types that directly implement the given interface or trait.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the interface or trait.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.implementors(id).into_iter()
                    .map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_subclasses ────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_subclasses".into(),
                description: "Get all transitive subclasses of the given class (follows Extends edges downward).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the class.".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "depth".into(),
                        kind:        ParamKind::Number,
                        description: "Max traversal depth (default: unlimited).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.all_subclasses(id, depth).into_iter()
                    .map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_superclasses ──────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_superclasses".into(),
                description: "Get the full superclass chain of the given class (follows Extends edges upward to the root).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the class.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.superclass_chain(id).into_iter()
                    .map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_methods ───────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_methods".into(),
                description: "Get all methods declared directly on a class, interface, or trait.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the type.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.methods_of(id).into_iter()
                    .map(|id| node_details(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_fields ────────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_fields".into(),
                description: "Get all fields, static fields, and constants declared on a type.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the type.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.fields_of(id).into_iter()
                    .map(|id| node_details(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_public_api ────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_public_api".into(),
                description: "Get all public members of a type (its external contract).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the type.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.public_api(id).into_iter()
                    .map(|id| node_details(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_coupling ──────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_coupling".into(),
                description: "Get afferent coupling (fan-in), efferent coupling (fan-out), and instability metric for a node. Instability = efferent / (afferent + efferent); 0 = stable, 1 = unstable.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the node.".into(),
                        required:    true,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let ca = explorer.afferent_coupling(id);
                let ce = explorer.efferent_coupling(id);
                let i  = explorer.instability(id);
                serde_json::to_string(&json!({
                    "qualified_name":      qname,
                    "afferent_coupling":   ca,
                    "efferent_coupling":   ce,
                    "instability":         if i.is_nan() { Value::Null } else { json!(i) },
                })).unwrap_or_default()
            },
        );
    }

    // ── get_hotspots ──────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_hotspots".into(),
                description: "Get the top-N most depended-upon nodes (highest incoming-edge count). Useful for identifying the core of the codebase.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "limit".into(),
                        kind:        ParamKind::Number,
                        description: "Number of hotspots to return (default: 20).".into(),
                        required:    false,
                    },
                ],
            },
            move |_args| {
                let v: Value = serde_json::from_str(_args).unwrap_or(Value::Null);
                let limit = v["limit"].as_u64().unwrap_or(20) as usize;
                let explorer = GraphExplorer::new(&g);
                let spots: Vec<Value> = explorer.hotspots(limit).into_iter()
                    .map(|(id, count)| {
                        let mut s = node_summary(&g, id);
                        if let Some(obj) = s.as_object_mut() {
                            obj.insert("incoming_edge_count".into(), json!(count));
                        }
                        s
                    })
                    .collect();
                serde_json::to_string(&spots).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_entry_points ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_entry_points".into(),
                description: "Get all public methods/functions with no internal callers — the true API entry points of the codebase.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "limit".into(),
                        kind:        ParamKind::Number,
                        description: "Maximum number of results (default: 50).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let limit = v["limit"].as_u64().unwrap_or(50) as usize;
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.entry_points().into_iter()
                    .take(limit)
                    .map(|id| node_summary(&g, id))
                    .collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_change_impact ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_change_impact".into(),
                description: "Get everything that may need to change if the given node changes (reverse closure over Calls, Instantiates, Reads, Writes, HasType, Returns, Extends, Implements edges).".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "qualified_name".into(),
                        kind:        ParamKind::String,
                        description: "The fully-qualified name of the node being changed.".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "depth".into(),
                        kind:        ParamKind::Number,
                        description: "Max traversal depth (default: unlimited).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() {
                    Some(q) => q,
                    None    => return r#"{"error":"missing 'qualified_name' parameter"}"#.into(),
                };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) {
                    Some(id) => id,
                    None     => return format!(r#"{{"error":"node not found: {}"}}"#, qname),
                };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.change_impact(id, depth).into_iter()
                    .map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_package_dependencies ──────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_package_dependencies".into(),
                description: "Get a collapsed package-to-package dependency view of the entire codebase (counts cross-package edges from Calls, Instantiates, Implements, Extends, Imports, References).".into(),
                parameters:  vec![],
            },
            move |_args| {
                let explorer = GraphExplorer::new(&g);
                let deps: Vec<Value> = explorer.package_dependency_graph().into_iter()
                    .map(|d| json!({
                        "from":       d.from,
                        "to":         d.to,
                        "edge_count": d.edge_count,
                    }))
                    .collect();
                serde_json::to_string(&deps).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_direct_callers ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_direct_callers".into(),
                description: "Get the immediate callers of a node (one hop, not transitive).".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "The fully-qualified name of the callee.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.direct_callers(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_direct_callees ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_direct_callees".into(),
                description: "Get the immediate callees of a node (one hop, not transitive).".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "The fully-qualified name of the caller.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.direct_callees(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_call_path ─────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_call_path".into(),
                description: "Find the shortest call chain between two nodes (inclusive). Returns null if unreachable.".into(),
                parameters:  vec![
                    ToolParameter { name: "from".into(), kind: ParamKind::String, description: "Qualified name of the starting node.".into(), required: true },
                    ToolParameter { name: "to".into(),   kind: ParamKind::String, description: "Qualified name of the target node.".into(),   required: true },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let from_qn = match v["from"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'from' parameter"}"#.into() };
                let to_qn   = match v["to"].as_str()   { Some(q) => q, None => return r#"{"error":"missing 'to' parameter"}"#.into() };
                let from_id = match g.find_by_qualified(from_qn) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, from_qn) };
                let to_id   = match g.find_by_qualified(to_qn)   { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, to_qn) };
                let explorer = GraphExplorer::new(&g);
                match explorer.call_path(from_id, to_id) {
                    None       => r#"{"path":null}"#.into(),
                    Some(path) => {
                        let nodes: Vec<Value> = path.into_iter().map(|id| node_summary(&g, id)).collect();
                        serde_json::to_string(&json!({ "path": nodes })).unwrap_or_default()
                    }
                }
            },
        );
    }

    // ── get_interface_hierarchy ───────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_interface_hierarchy".into(),
                description: "Get interfaces that transitively extend the given interface (sub-interfaces).".into(),
                parameters:  vec![
                    ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the interface.".into(), required: true },
                    ToolParameter { name: "depth".into(), kind: ParamKind::Number, description: "Max traversal depth (default: unlimited).".into(), required: false },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.interface_hierarchy(id, depth).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_overriders ────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_overriders".into(),
                description: "Get all methods in subclasses that override the given method.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the base method.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.overriders(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_what_overrides ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_what_overrides".into(),
                description: "Get the parent method that the given method overrides (one hop upward), if any.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the overriding method.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                match explorer.what_overrides(id) {
                    None         => r#"{"overrides":null}"#.into(),
                    Some(parent) => serde_json::to_string(&json!({ "overrides": node_summary(&g, parent) })).unwrap_or_default(),
                }
            },
        );
    }

    // ── get_unimplemented_methods ─────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_unimplemented_methods".into(),
                description: "Get interface methods that a class inherits but does not concretely implement (abstract-gap detection).".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the class.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.unimplemented_interface_methods(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_readers_of ────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_readers_of".into(),
                description: "Get all methods that read the given field.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the field.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.readers_of(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_writers_of ────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_writers_of".into(),
                description: "Get all methods that write/assign the given field.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the field.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.writers_of(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_fields_read_by ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_fields_read_by".into(),
                description: "Get all fields/variables read inside the given method.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the method.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.fields_read_by(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_fields_written_by ─────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_fields_written_by".into(),
                description: "Get all fields/variables written inside the given method.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the method.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.fields_written_by(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_unused_fields ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_unused_fields".into(),
                description: "Get fields of a type that have no Reads or Writes edges (potentially dead).".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the type.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.unused_fields(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_nodes_in_package ──────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_nodes_in_package".into(),
                description: "Get all nodes whose qualified name starts with the given package prefix.".into(),
                parameters:  vec![
                    ToolParameter { name: "package_prefix".into(), kind: ParamKind::String, description: "The package prefix to filter by (e.g. 'com.example.service').".into(), required: true },
                    ToolParameter { name: "limit".into(), kind: ParamKind::Number, description: "Maximum number of results (default: 100).".into(), required: false },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let prefix = match v["package_prefix"].as_str() { Some(p) => p, None => return r#"{"error":"missing 'package_prefix' parameter"}"#.into() };
                let limit = v["limit"].as_u64().unwrap_or(100) as usize;
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.nodes_in_package(prefix).into_iter().take(limit).map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_direct_imports ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_direct_imports".into(),
                description: "Get nodes that the given node directly imports (one hop).".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the source node.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.direct_imports(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_direct_importers ──────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_direct_importers".into(),
                description: "Get nodes that directly import the given node.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the imported node.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.direct_importers(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_import_closure ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_import_closure".into(),
                description: "Get the transitive import closure of a node (all resolved imports reachable from it).".into(),
                parameters:  vec![
                    ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the source node.".into(), required: true },
                    ToolParameter { name: "depth".into(), kind: ParamKind::Number, description: "Max traversal depth (default: unlimited).".into(), required: false },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.import_closure(id, depth).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_instantiators ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_instantiators".into(),
                description: "Get all methods/locations that instantiate the given class.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the class.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.instantiators(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_types_instantiated_by ─────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_types_instantiated_by".into(),
                description: "Get all classes instantiated within the given method.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the method.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.types_instantiated_by(id).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_nodes_with_attribute ──────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_nodes_with_attribute".into(),
                description: "Get all nodes that carry a specific attribute/annotation (e.g. '@Override', '@Deprecated').".into(),
                parameters:  vec![
                    ToolParameter { name: "attribute".into(), kind: ParamKind::String, description: "The attribute string to search for.".into(), required: true },
                    ToolParameter { name: "limit".into(), kind: ParamKind::Number, description: "Maximum number of results (default: 50).".into(), required: false },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let attr = match v["attribute"].as_str() { Some(a) => a, None => return r#"{"error":"missing 'attribute' parameter"}"#.into() };
                let limit = v["limit"].as_u64().unwrap_or(50) as usize;
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.nodes_with_attribute(attr).into_iter().take(limit).map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_methods_throwing ──────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_methods_throwing".into(),
                description: "Get all methods that declare a Throws edge to the given exception type.".into(),
                parameters:  vec![ToolParameter { name: "exception_name".into(), kind: ParamKind::String, description: "Simple or qualified exception name (e.g. 'IOException').".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let exc = match v["exception_name"].as_str() { Some(e) => e, None => return r#"{"error":"missing 'exception_name' parameter"}"#.into() };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.methods_throwing(exc).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_async_call_chain ──────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_async_call_chain".into(),
                description: "Get the transitive async call tree from a given async method (follows Awaits edges).".into(),
                parameters:  vec![
                    ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the async method.".into(), required: true },
                    ToolParameter { name: "depth".into(), kind: ParamKind::Number, description: "Max traversal depth (default: unlimited).".into(), required: false },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let depth = v["depth"].as_u64().map(|n| n as usize);
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.async_call_chain(id, depth).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_parameters_of ─────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_parameters_of".into(),
                description: "Get all parameters declared on a function or method.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the function or method.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.parameters_of(id).into_iter().map(|id| node_details(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_parent_of ─────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_parent_of".into(),
                description: "Get the direct parent of a node in the Contains hierarchy (e.g. the class that contains a method).".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the node.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                match explorer.parent_of(id) {
                    None    => r#"{"parent":null}"#.into(),
                    Some(p) => serde_json::to_string(&json!({ "parent": node_summary(&g, p) })).unwrap_or_default(),
                }
            },
        );
    }

    // ── get_external_dependencies ─────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_external_dependencies".into(),
                description: "Get all ExternalPackage nodes — third-party libraries and framework dependencies declared in the project manifest.".into(),
                parameters:  vec![],
            },
            move |_args| {
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.external_dependencies().into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_usages_of_type ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_usages_of_type".into(),
                description: "Get all nodes that reference a type via HasType, Returns, Instantiates, or References edges.".into(),
                parameters:  vec![
                    ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the type.".into(), required: true },
                    ToolParameter { name: "limit".into(), kind: ParamKind::Number, description: "Maximum number of results (default: 50).".into(), required: false },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let limit = v["limit"].as_u64().unwrap_or(50) as usize;
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.usages_of_type(id).into_iter().take(limit).map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_package_cycles ────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_package_cycles".into(),
                description: "Detect circular dependencies between packages. Returns each cycle as an ordered list of package names.".into(),
                parameters:  vec![],
            },
            move |_args| {
                let explorer = GraphExplorer::new(&g);
                let cycles: Vec<Value> = explorer.package_cycles().into_iter()
                    .map(|c| Value::Array(c.into_iter().map(|s| json!(s)).collect()))
                    .collect();
                serde_json::to_string(&json!({ "cycles": cycles })).unwrap_or_default()
            },
        );
    }

    // ── get_cohesion ──────────────────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_cohesion".into(),
                description: "Compute the Lack of Cohesion of Methods (LCOM) for a type. 0 = perfectly cohesive, 1 = completely non-cohesive.".into(),
                parameters:  vec![ToolParameter { name: "qualified_name".into(), kind: ParamKind::String, description: "Qualified name of the type.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qname = match v["qualified_name"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name' parameter"}"#.into() };
                let id = match g.find_by_qualified(qname) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qname) };
                let explorer = GraphExplorer::new(&g);
                let lcom = explorer.lcom(id);
                serde_json::to_string(&json!({ "qualified_name": qname, "lcom": if lcom.is_nan() { Value::Null } else { json!(lcom) } })).unwrap_or_default()
            },
        );
    }

    // ── get_exception_propagation ─────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_exception_propagation".into(),
                description: "Get all methods that declare or propagate a given exception through the call chain.".into(),
                parameters:  vec![ToolParameter { name: "exception_name".into(), kind: ParamKind::String, description: "Simple or qualified exception name.".into(), required: true }],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let exc = match v["exception_name"].as_str() { Some(e) => e, None => return r#"{"error":"missing 'exception_name' parameter"}"#.into() };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.exception_propagation(exc).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_shared_dependencies ───────────────────────────────────────────────
    {
        let g = Arc::clone(&graph);
        tools.register(
            ToolDefinition {
                name:        "get_shared_dependencies".into(),
                description: "Get types that both node A and node B depend on (intersection of their efferent dependency sets).".into(),
                parameters:  vec![
                    ToolParameter { name: "qualified_name_a".into(), kind: ParamKind::String, description: "Qualified name of the first node.".into(),  required: true },
                    ToolParameter { name: "qualified_name_b".into(), kind: ParamKind::String, description: "Qualified name of the second node.".into(), required: true },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let qn_a = match v["qualified_name_a"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name_a' parameter"}"#.into() };
                let qn_b = match v["qualified_name_b"].as_str() { Some(q) => q, None => return r#"{"error":"missing 'qualified_name_b' parameter"}"#.into() };
                let id_a = match g.find_by_qualified(qn_a) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qn_a) };
                let id_b = match g.find_by_qualified(qn_b) { Some(id) => id, None => return format!(r#"{{"error":"node not found: {}"}}"#, qn_b) };
                let explorer = GraphExplorer::new(&g);
                let nodes: Vec<Value> = explorer.shared_dependencies(id_a, id_b).into_iter().map(|id| node_summary(&g, id)).collect();
                serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
            },
        );
    }

    // ── get_file_source ───────────────────────────────────────────────────────
    {
        let fs = Rc::clone(&fs);
        tools.register(
            ToolDefinition {
                name:        "get_file_source".into(),
                description: "Read the raw source code of a file, optionally restricted to a line range. \
                              Use sparingly — only when graph metadata is insufficient. Lines are 1-indexed.".into(),
                parameters:  vec![
                    ToolParameter {
                        name:        "file".into(),
                        kind:        ParamKind::String,
                        description: "Absolute or graph-relative path to the source file.".into(),
                        required:    true,
                    },
                    ToolParameter {
                        name:        "start_line".into(),
                        kind:        ParamKind::Number,
                        description: "First line to return, 1-indexed (default: 1).".into(),
                        required:    false,
                    },
                    ToolParameter {
                        name:        "end_line".into(),
                        kind:        ParamKind::Number,
                        description: "Last line to return, inclusive (default: end of file).".into(),
                        required:    false,
                    },
                ],
            },
            move |args| {
                let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
                let file = match v["file"].as_str() {
                    Some(f) => f,
                    None    => return r#"{"error":"missing 'file' parameter"}"#.into(),
                };
                let content = match fs.read(file) {
                    Some(c) => c,
                    None    => return format!(r#"{{"error":"file not found: {}"}}"#, file),
                };
                let lines: Vec<&str> = content.lines().collect();
                let total = lines.len();
                let start = v["start_line"].as_u64().map(|n| (n as usize).saturating_sub(1)).unwrap_or(0);
                let end   = v["end_line"].as_u64().map(|n| (n as usize).min(total)).unwrap_or(total);
                let start = start.min(total);
                let end   = end.max(start);
                let numbered: Vec<Value> = lines[start..end].iter().enumerate()
                    .map(|(i, line)| json!({ "line": start + i + 1, "content": line }))
                    .collect();
                serde_json::to_string(&json!({
                    "file":        file,
                    "total_lines": total,
                    "start_line":  start + 1,
                    "end_line":    end,
                    "lines":       numbered,
                })).unwrap_or_else(|_| "{}".into())
            },
        );
    }
}
