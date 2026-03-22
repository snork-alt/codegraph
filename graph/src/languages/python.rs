use tree_sitter::{Node as TsNode, Parser};

use crate::graph::{
    DependencyGraph, Edge, EdgeKind, EdgeTarget, Language, Node, NodeId, NodeKind, Span, Visibility,
};
use crate::parser::{hash_source, LanguageExtractor};

// ─── Low-level helpers ────────────────────────────────────────────────────────

fn ts_span(node: &TsNode) -> Span {
    let s = node.start_position();
    let e = node.end_position();
    Span::new(s.row as u32, s.column as u32, e.row as u32, e.column as u32)
}

fn node_text<'a>(node: &TsNode, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn field_text<'a>(parent: &TsNode, field: &str, source: &'a [u8]) -> Option<&'a str> {
    parent.child_by_field_name(field).map(|n| node_text(&n, source))
}

fn find_child<'a>(parent: &'a TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    let mut c = parent.walk();
    parent.children(&mut c).find(|n| n.kind() == kind)
}

fn children_of_kind<'a>(parent: &'a TsNode<'a>, kind: &str) -> Vec<TsNode<'a>> {
    let mut c = parent.walk();
    parent.children(&mut c).filter(|n| n.kind() == kind).collect()
}

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() { name.to_owned() } else { format!("{}.{}", scope, name) }
}

/// In tree-sitter-python 0.23, async functions are `function_definition` nodes
/// with an unnamed `async` keyword child (no separate `async_function_definition` node).
fn has_async_keyword(node: &TsNode, source: &[u8]) -> bool {
    let mut c = node.walk();
    node.children(&mut c)
        .any(|n| !n.is_named() && node_text(&n, source) == "async")
}

// ─── Context ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Ctx<'a> {
    source: &'a [u8],
    file:   &'a str,
    scope:  String,
}

impl<'a> Ctx<'a> {
    fn new(source: &'a [u8], file: &'a str) -> Self {
        Self { source, file, scope: String::new() }
    }
    fn child_scope(&self, name: &str) -> Self {
        Self { scope: qualify(&self.scope, name), ..self.clone() }
    }
}

// ─── Extractor ────────────────────────────────────────────────────────────────

pub struct PythonExtractor;

impl LanguageExtractor for PythonExtractor {
    fn language(&self) -> Language { Language::Python }

    fn extract(&self, source: &str, file: &str, graph: &mut DependencyGraph) {
        let src = source.as_bytes();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("failed to load Python grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None    => return,
        };

        let simple_name = std::path::Path::new(file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file);
        let mut file_node = Node::new(
            0, NodeKind::File, simple_name, file, file,
            Span::new(0, 0, 0, 0), Language::Python,
        );
        file_node.visibility = Visibility::Public;
        file_node.hash       = Some(hash_source(source));
        let file_id = graph.add_node(file_node);

        let ctx = Ctx::new(src, file);
        walk_module(&tree.root_node(), graph, &ctx, file_id);
    }
}

// ─── Module-level walk ────────────────────────────────────────────────────────

fn walk_module(root: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        dispatch_top(&child, graph, ctx, parent_id, &[]);
    }
}

fn dispatch_top(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId, decorators: &[String]) {
    match node.kind() {
        "import_statement"      => handle_import(node, graph, ctx, parent_id),
        "import_from_statement" => handle_import_from(node, graph, ctx, parent_id),
        "class_definition"      => handle_class(node, graph, ctx, parent_id, decorators),
        "function_definition" => {
            let is_async = has_async_keyword(node, ctx.source);
            handle_function(node, graph, ctx, parent_id, decorators, is_async);
        }
        "decorated_definition"  => handle_decorated(node, graph, ctx, parent_id),
        "expression_statement"  => {
            // Could be a module-level assignment.
            if let Some(inner) = node.named_child(0) {
                if inner.kind() == "assignment" {
                    handle_module_assignment(&inner, graph, ctx, parent_id);
                }
            }
        }
        "assignment" => handle_module_assignment(node, graph, ctx, parent_id),
        _ => {}
    }
}

// ─── Decorated definitions ────────────────────────────────────────────────────

fn handle_decorated(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mut decorators: Vec<String> = vec![];
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "decorator" => {
                let text = node_text(&child, ctx.source)
                    .trim_start_matches('@')
                    .to_owned();
                decorators.push(format!("@{}", text));
            }
            "class_definition" => handle_class(&child, graph, ctx, parent_id, &decorators),
            "function_definition" => {
                let is_async = has_async_keyword(&child, ctx.source);
                handle_function(&child, graph, ctx, parent_id, &decorators, is_async);
            }
            _ => {}
        }
    }
}

// ─── Import ───────────────────────────────────────────────────────────────────

fn handle_import(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                let path = node_text(&child, ctx.source).to_owned();
                let simple = path.split('.').last().unwrap_or(&path).to_owned();
                let mut imp = Node::new(
                    0, NodeKind::Import, &simple, &path,
                    ctx.file, ts_span(&child), Language::Python,
                );
                let imp_id = graph.add_node(imp);
                graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(&child));
                graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(path), ts_span(&child));
            }
            "aliased_import" => {
                let name = field_text(&child, "name", ctx.source).unwrap_or("").to_owned();
                let alias = field_text(&child, "alias", ctx.source).unwrap_or("").to_owned();
                let display = if alias.is_empty() { name.clone() } else { alias.clone() };
                let mut imp = Node::new(
                    0, NodeKind::Import, &display, &name,
                    ctx.file, ts_span(&child), Language::Python,
                );
                if !alias.is_empty() {
                    imp.metadata.insert("alias".into(), alias);
                }
                let imp_id = graph.add_node(imp);
                graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(&child));
                graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(name), ts_span(&child));
            }
            _ => {}
        }
    }
}

fn handle_import_from(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let module = node.child_by_field_name("module_name")
        .map(|n| node_text(&n, ctx.source).to_owned())
        .unwrap_or_default();

    // In tree-sitter-python 0.23, `from X import a, b, c` produces multiple `name` fields
    // (each a dotted_name or aliased_import). Collect them all via cursor.
    let mut name_nodes: Vec<TsNode> = Vec::new();
    let has_wildcard = find_child(node, "wildcard_import").is_some();

    if !has_wildcard {
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                if cursor.field_name() == Some("name") {
                    name_nodes.push(cursor.node());
                }
                if !cursor.goto_next_sibling() { break; }
            }
        }
    }

    if has_wildcard {
        let path = format!("{}.*", module);
        let mut imp = Node::new(
            0, NodeKind::Import, "*", &path,
            ctx.file, ts_span(node), Language::Python,
        );
        imp.metadata.insert("wildcard".into(), "true".into());
        let imp_id = graph.add_node(imp);
        graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(node));
        graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(module), ts_span(node));
    } else if name_nodes.is_empty() && !module.is_empty() {
        // `from . import X` style with relative import — no named fields found
        let imp = Node::new(
            0, NodeKind::Import, &module, &module,
            ctx.file, ts_span(node), Language::Python,
        );
        let imp_id = graph.add_node(imp);
        graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(node));
        graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(module), ts_span(node));
    } else {
        for nn in name_nodes {
            match nn.kind() {
                "dotted_name" | "identifier" => {
                    let name = node_text(&nn, ctx.source).to_owned();
                    let qpath = format!("{}.{}", module, name);
                    let imp = Node::new(
                        0, NodeKind::Import, &name, &qpath,
                        ctx.file, ts_span(&nn), Language::Python,
                    );
                    let imp_id = graph.add_node(imp);
                    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(&nn));
                    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(qpath), ts_span(&nn));
                }
                "aliased_import" => {
                    let orig = field_text(&nn, "name", ctx.source).unwrap_or("").to_owned();
                    let alias = field_text(&nn, "alias", ctx.source).unwrap_or("").to_owned();
                    let display = if alias.is_empty() { orig.clone() } else { alias.clone() };
                    let qpath = format!("{}.{}", module, orig);
                    let mut imp = Node::new(
                        0, NodeKind::Import, &display, &qpath,
                        ctx.file, ts_span(&nn), Language::Python,
                    );
                    if !alias.is_empty() {
                        imp.metadata.insert("alias".into(), alias);
                    }
                    let imp_id = graph.add_node(imp);
                    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(&nn));
                    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(qpath), ts_span(&nn));
                }
                _ => {}
            }
        }
    }
}

// ─── Class ────────────────────────────────────────────────────────────────────

fn handle_class(
    node:       &TsNode,
    graph:      &mut DependencyGraph,
    ctx:        &Ctx,
    parent_id:  NodeId,
    decorators: &[String],
) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);

    let mut cls = Node::new(0, NodeKind::Class, &name, &qname, ctx.file, ts_span(node), Language::Python);
    cls.visibility = Visibility::Public;
    cls.attributes = decorators.to_vec();
    let cls_id = graph.add_node(cls);
    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(cls_id), ts_span(node));

    // Superclasses from argument_list (e.g. `class Foo(Bar, Baz):`)
    if let Some(superclasses) = node.child_by_field_name("superclasses") {
        let mut cursor = superclasses.walk();
        for base in superclasses.children(&mut cursor) {
            if !base.is_named() { continue; }
            let base_name = node_text(&base, ctx.source).to_owned();
            if base_name.is_empty() || base_name == "object" { continue; }
            graph.add_edge_simple(EdgeKind::Extends, cls_id, EdgeTarget::Unresolved(base_name), ts_span(&base));
        }
    }

    // Decorates edges
    for dec in decorators {
        let dec_name = dec.trim_start_matches('@').to_owned();
        graph.add_edge_simple(EdgeKind::Decorates, cls_id, EdgeTarget::Unresolved(dec_name), ts_span(node));
    }

    // Walk class body
    let child_ctx = ctx.child_scope(&name);
    if let Some(body) = node.child_by_field_name("body") {
        walk_class_body(&body, graph, &child_ctx, cls_id);
    }
}

fn walk_class_body(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, cls_id: NodeId) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                let is_async = has_async_keyword(&child, ctx.source);
                handle_method(&child, graph, ctx, cls_id, &[], is_async);
            }
            "decorated_definition"      => handle_decorated_method(&child, graph, ctx, cls_id),
            "expression_statement" => {
                // Class-level annotations like `x: int = 0`
                if let Some(inner) = child.named_child(0) {
                    if inner.kind() == "assignment" || inner.kind() == "annotated_assignment" {
                        handle_class_assignment(&inner, graph, ctx, cls_id);
                    }
                }
            }
            "assignment" | "annotated_assignment" => {
                handle_class_assignment(&child, graph, ctx, cls_id);
            }
            _ => {}
        }
    }
}

fn handle_decorated_method(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, cls_id: NodeId) {
    let mut decorators: Vec<String> = vec![];
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "decorator" => {
                let text = node_text(&child, ctx.source)
                    .trim_start_matches('@')
                    .to_owned();
                decorators.push(format!("@{}", text));
            }
            "function_definition" => {
                let is_async = has_async_keyword(&child, ctx.source);
                handle_method(&child, graph, ctx, cls_id, &decorators, is_async);
            }
            _ => {}
        }
    }
}

// ─── Class-level annotations / assignments ────────────────────────────────────

fn handle_class_assignment(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, cls_id: NodeId) {
    // `x: int = 0` → annotated_assignment; `x = 0` → assignment
    let name = if node.kind() == "annotated_assignment" {
        field_text(node, "variable", ctx.source).unwrap_or("").to_owned()
    } else {
        node.child_by_field_name("left")
            .map(|n| node_text(&n, ctx.source).to_owned())
            .unwrap_or_default()
    };
    if name.is_empty() || name.starts_with("self.") { return; }

    let type_text = if node.kind() == "annotated_assignment" {
        field_text(node, "annotation", ctx.source).map(str::to_owned)
    } else {
        None
    };

    let qname = qualify(&ctx.scope, &name);
    let mut f = Node::new(0, NodeKind::Field, &name, &qname, ctx.file, ts_span(node), Language::Python);
    f.visibility      = Visibility::Public;
    f.type_annotation = type_text;
    let fid = graph.add_node(f);
    graph.add_edge_simple(EdgeKind::Contains, cls_id, EdgeTarget::Resolved(fid), ts_span(node));
}

// ─── Method ───────────────────────────────────────────────────────────────────

fn handle_method(
    node:       &TsNode,
    graph:      &mut DependencyGraph,
    ctx:        &Ctx,
    cls_id:     NodeId,
    decorators: &[String],
    is_async:   bool,
) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);

    let is_static      = decorators.iter().any(|d| d == "@staticmethod");
    let is_classmethod = decorators.iter().any(|d| d == "@classmethod");
    let is_property    = decorators.iter().any(|d| d == "@property");
    let is_constructor = name == "__init__";

    let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(node), Language::Python);
    m.visibility    = if name.starts_with("__") && !name.ends_with("__") {
        Visibility::Private
    } else if name.starts_with('_') {
        Visibility::Private
    } else {
        Visibility::Public
    };
    m.is_async      = is_async;
    m.is_constructor = is_constructor;
    m.attributes    = decorators.to_vec();
    if let Some(ret) = field_text(node, "return_type", ctx.source) {
        m.type_annotation = Some(ret.trim_start_matches("->").trim().to_owned());
    }
    if is_static      { m.metadata.insert("static".into(), "true".into()); }
    if is_classmethod { m.metadata.insert("classmethod".into(), "true".into()); }
    if is_property    { m.metadata.insert("property".into(), "true".into()); }

    let mid = graph.add_node(m);
    graph.add_edge_simple(EdgeKind::Contains, cls_id, EdgeTarget::Resolved(mid), ts_span(node));

    // Decorates edges
    for dec in decorators {
        let dec_name = dec.trim_start_matches('@').to_owned();
        graph.add_edge_simple(EdgeKind::Decorates, mid, EdgeTarget::Unresolved(dec_name), ts_span(node));
    }

    // Return type edge
    if let Some(ret) = field_text(node, "return_type", ctx.source) {
        let ret_clean = ret.trim_start_matches("->").trim().to_owned();
        if !ret_clean.is_empty() && ret_clean != "None" {
            graph.add_edge_simple(EdgeKind::Returns, mid, EdgeTarget::Unresolved(ret_clean), ts_span(node));
        }
    }

    // Parameters (skip 'self' and 'cls')
    let child_ctx = ctx.child_scope(&name);
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_parameters(&params, graph, &child_ctx, mid, is_static || is_classmethod);
    }

    // Body: extract self.x = ... assignments (only in __init__) and calls.
    if let Some(body) = node.child_by_field_name("body") {
        if is_constructor {
            extract_self_assignments(&body, graph, ctx, cls_id, mid);
        }
        walk_fn_body(&body, graph, &child_ctx, mid);
    }
}

// ─── Module-level assignment ──────────────────────────────────────────────────

fn handle_module_assignment(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = node.child_by_field_name("left")
        .map(|n| node_text(&n, ctx.source).to_owned())
        .unwrap_or_default();
    if name.is_empty() || !name.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false) {
        return;
    }

    let qname = qualify(&ctx.scope, &name);
    // All-uppercase names → Constant; others → GlobalVariable.
    let kind = if name.chars().all(|c| c.is_uppercase() || c == '_') {
        NodeKind::Constant
    } else {
        NodeKind::GlobalVariable
    };
    let mut v = Node::new(0, kind, &name, &qname, ctx.file, ts_span(node), Language::Python);
    v.visibility = Visibility::Public;
    let vid = graph.add_node(v);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(vid), ts_span(node));
}

// ─── Module-level function ────────────────────────────────────────────────────

fn handle_function(
    node:       &TsNode,
    graph:      &mut DependencyGraph,
    ctx:        &Ctx,
    parent_id:  NodeId,
    decorators: &[String],
    is_async:   bool,
) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);

    let mut f = Node::new(0, NodeKind::Function, &name, &qname, ctx.file, ts_span(node), Language::Python);
    f.visibility = if name.starts_with('_') { Visibility::Private } else { Visibility::Public };
    f.is_async   = is_async;
    f.attributes = decorators.to_vec();
    if let Some(ret) = field_text(node, "return_type", ctx.source) {
        f.type_annotation = Some(ret.trim_start_matches("->").trim().to_owned());
    }
    let fid = graph.add_node(f);
    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(fid), ts_span(node));

    if let Some(ret) = field_text(node, "return_type", ctx.source) {
        let ret_clean = ret.trim_start_matches("->").trim().to_owned();
        if !ret_clean.is_empty() && ret_clean != "None" {
            graph.add_edge_simple(EdgeKind::Returns, fid, EdgeTarget::Unresolved(ret_clean), ts_span(node));
        }
    }

    let child_ctx = ctx.child_scope(&name);
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_parameters(&params, graph, &child_ctx, fid, false);
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk_fn_body(&body, graph, &child_ctx, fid);
    }
}

// ─── Parameters ───────────────────────────────────────────────────────────────

fn handle_parameters(
    params:      &TsNode,
    graph:       &mut DependencyGraph,
    ctx:         &Ctx,
    fn_id:       NodeId,
    skip_first:  bool, // skip self/cls
) {
    let mut cursor = params.walk();
    let mut first = true;
    for param in params.children(&mut cursor) {
        match param.kind() {
            "identifier" => {
                if first && skip_first { first = false; continue; }
                let name = node_text(&param, ctx.source).to_owned();
                if name == "self" || name == "cls" { first = false; continue; }
                if name.is_empty() { continue; }
                emit_param(&param, graph, ctx, fn_id, &name, None);
                first = false;
            }
            "typed_parameter" => {
                if first && skip_first { first = false; continue; }
                let name = find_child(&param, "identifier")
                    .map(|n| node_text(&n, ctx.source).to_owned())
                    .unwrap_or_default();
                if name == "self" || name == "cls" { first = false; continue; }
                if name.is_empty() { continue; }
                let type_text = param.child_by_field_name("type")
                    .map(|n| node_text(&n, ctx.source).to_owned());
                emit_param(&param, graph, ctx, fn_id, &name, type_text);
                first = false;
            }
            "default_parameter" => {
                if first && skip_first { first = false; continue; }
                let name = field_text(&param, "name", ctx.source).unwrap_or("").to_owned();
                if name == "self" || name == "cls" { first = false; continue; }
                if name.is_empty() { continue; }
                emit_param(&param, graph, ctx, fn_id, &name, None);
                first = false;
            }
            "typed_default_parameter" => {
                if first && skip_first { first = false; continue; }
                let name = field_text(&param, "name", ctx.source).unwrap_or("").to_owned();
                if name == "self" || name == "cls" { first = false; continue; }
                if name.is_empty() { continue; }
                let type_text = field_text(&param, "type", ctx.source).map(str::to_owned);
                emit_param(&param, graph, ctx, fn_id, &name, type_text);
                first = false;
            }
            _ => {}
        }
    }
}

fn emit_param(
    node:      &TsNode,
    graph:     &mut DependencyGraph,
    ctx:       &Ctx,
    fn_id:     NodeId,
    name:      &str,
    type_text: Option<String>,
) {
    let qname = qualify(&ctx.scope, name);
    let mut p = Node::new(0, NodeKind::Parameter, name, &qname, ctx.file, ts_span(node), Language::Python);
    p.type_annotation = type_text.clone();
    let pid = graph.add_node(p);
    graph.add_edge_simple(EdgeKind::HasParameter, fn_id, EdgeTarget::Resolved(pid), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::HasType, pid, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

// ─── Self-assignment extraction (for __init__) ────────────────────────────────

fn extract_self_assignments(
    body:    &TsNode,
    graph:   &mut DependencyGraph,
    ctx:     &Ctx,    // class scope
    cls_id:  NodeId,
    init_id: NodeId,
) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "expression_statement" => {
                if let Some(inner) = child.named_child(0) {
                    if inner.kind() == "assignment" {
                        try_extract_self_field(&inner, graph, ctx, cls_id, init_id);
                    }
                }
            }
            "assignment" => try_extract_self_field(&child, graph, ctx, cls_id, init_id),
            _ => {}
        }
    }
}

fn try_extract_self_field(
    node:    &TsNode,
    graph:   &mut DependencyGraph,
    ctx:     &Ctx,
    cls_id:  NodeId,
    init_id: NodeId,
) {
    let lhs = node.child_by_field_name("left");
    let Some(lhs) = lhs else { return };

    // lhs must be an attribute like `self.x`
    if lhs.kind() != "attribute" { return; }
    let obj  = lhs.child_by_field_name("object").map(|n| node_text(&n, ctx.source)).unwrap_or("");
    let attr = lhs.child_by_field_name("attribute").map(|n| node_text(&n, ctx.source)).unwrap_or("");

    if obj != "self" || attr.is_empty() { return; }

    // Avoid duplicate Field nodes if already declared at class body level.
    let qname = qualify(&ctx.scope, attr);
    if graph.by_qualified.contains_key(&qname) { return; }

    let mut f = Node::new(0, NodeKind::Field, attr, &qname, ctx.file, ts_span(node), Language::Python);
    f.visibility = if attr.starts_with('_') { Visibility::Private } else { Visibility::Public };
    let fid = graph.add_node(f);
    graph.add_edge_simple(EdgeKind::Contains, cls_id, EdgeTarget::Resolved(fid), ts_span(node));
    graph.add_edge_simple(EdgeKind::Writes, init_id, EdgeTarget::Resolved(fid), ts_span(node));
}

// ─── Function body ────────────────────────────────────────────────────────────

fn walk_fn_body(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_expr(&child, graph, ctx, fn_id);
    }
}

fn dispatch_expr(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    match node.kind() {
        "call" => handle_call(node, graph, ctx, fn_id),
        "await" => {
            // tree-sitter-python 0.23: await node has no "value" field, just named_child(0)
            if let Some(val) = node.named_child(0) {
                if val.kind() == "call" {
                    handle_call(&val, graph, ctx, fn_id);
                    let callee = val.child_by_field_name("function")
                        .map(|n| node_text(&n, ctx.source).to_owned())
                        .unwrap_or_default();
                    if !callee.is_empty() {
                        graph.add_edge_simple(EdgeKind::Awaits, fn_id, EdgeTarget::Unresolved(callee), ts_span(node));
                    }
                }
            }
        }
        "expression_statement" => {
            if let Some(inner) = node.named_child(0) {
                dispatch_expr(&inner, graph, ctx, fn_id);
            }
        }
        _ => walk_fn_body(node, graph, ctx, fn_id),
    }
}

fn handle_call(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    if let Some(func) = node.child_by_field_name("function") {
        let callee = node_text(&func, ctx.source).to_owned();
        if !callee.is_empty() {
            let arity = node.child_by_field_name("arguments")
                .map(|args| {
                    let mut c = args.walk();
                    args.children(&mut c).filter(|n| n.is_named()).count() as u32
                })
                .unwrap_or(0);
            let mut edge = Edge::new(0, EdgeKind::Calls, fn_id, EdgeTarget::Unresolved(callee), ts_span(node));
            edge.call_arity = Some(arity);
            graph.add_edge(edge);
        }
    }
    if let Some(args) = node.child_by_field_name("arguments") {
        walk_fn_body(&args, graph, ctx, fn_id);
    }
}
