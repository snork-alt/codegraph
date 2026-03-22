use tree_sitter::{Node as TsNode, Parser};

use crate::graph::{
    DependencyGraph, Edge, EdgeKind, EdgeTarget, Language, Node, NodeId, NodeKind, Span, Visibility,
};
use crate::parser::{hash_source, LanguageExtractor};

// ─── Span / text helpers ──────────────────────────────────────────────────────

fn ts_span(node: &TsNode) -> Span {
    let s = node.start_position();
    let e = node.end_position();
    Span::new(s.row as u32, s.column as u32, e.row as u32, e.column as u32)
}

fn node_text<'a>(node: &TsNode, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn field_text<'a>(parent: &TsNode, field: &str, src: &'a [u8]) -> Option<&'a str> {
    parent.child_by_field_name(field).map(|n| node_text(&n, src))
}

fn find_child<'a>(parent: &'a TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    let mut c = parent.walk();
    parent.children(&mut c).find(|n| n.kind() == kind)
}

fn children_of_kind<'a>(parent: &'a TsNode<'a>, kind: &str) -> Vec<TsNode<'a>> {
    let mut c = parent.walk();
    parent.children(&mut c).filter(|n| n.kind() == kind).collect()
}

// ─── Rust-specific helpers ────────────────────────────────────────────────────

/// Visibility: `pub` children are present if public. No child = private.
fn get_visibility(node: &TsNode, src: &[u8]) -> Visibility {
    match find_child(node, "visibility_modifier") {
        None => Visibility::Private,
        Some(v) => {
            let text = node_text(&v, src);
            if text.contains("pub(crate)") || text.contains("pub(self)") {
                Visibility::Internal
            } else if text.contains("pub(super)") {
                Visibility::Internal
            } else {
                Visibility::Public
            }
        }
    }
}

/// Collect `#[...]` attribute text from preceding `attribute_item` siblings.
/// Called by `walk_items` which passes accumulated attrs for each item.
fn attribute_texts(attr_nodes: &[TsNode], src: &[u8]) -> Vec<String> {
    attr_nodes
        .iter()
        .map(|n| node_text(n, src).trim().to_owned())
        .collect()
}

/// Detect `async` from a `function_modifiers` child node.
fn is_async(fn_node: &TsNode, src: &[u8]) -> bool {
    find_child(fn_node, "function_modifiers")
        .map(|m| node_text(&m, src).contains("async"))
        .unwrap_or(false)
        // `async` can also appear as a direct unnamed child of function_item
        || {
            let mut c = fn_node.walk();
            fn_node
                .children(&mut c)
                .any(|ch| ch.kind() == "async")
        }
}

/// Extract generic type-parameter names from a `type_parameters` node.
fn collect_type_params(tp: Option<TsNode>, src: &[u8]) -> Vec<String> {
    let Some(tp) = tp else { return vec![] };
    children_of_kind(&tp, "type_parameter")
        .iter()
        .filter_map(|p| {
            p.child_by_field_name("name")
                .map(|n| node_text(&n, src).to_owned())
        })
        .collect()
}

/// Extract raw bound strings from type_parameter / where_clause nodes.
fn collect_type_bounds(tp: Option<TsNode>, where_node: Option<TsNode>, src: &[u8]) -> Vec<String> {
    let mut bounds = Vec::new();
    // from inline bounds: `T: Display + Clone`
    if let Some(tp) = tp {
        for p in children_of_kind(&tp, "type_parameter") {
            if p.child_by_field_name("bounds").is_some() {
                bounds.push(node_text(&p, src).to_owned());
            }
        }
    }
    // from where clause
    if let Some(wc) = where_node {
        let mut c = wc.walk();
        for pred in wc.children(&mut c) {
            if pred.kind() == "where_predicate" {
                bounds.push(node_text(&pred, src).to_owned());
            }
        }
    }
    bounds
}

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() { name.to_owned() } else { format!("{}::{}", scope, name) }
}

// ─── use-declaration flattening ───────────────────────────────────────────────

/// Returns a list of `(full_path, alias)` for a use declaration argument node.
fn flatten_use(node: &TsNode, prefix: &str, src: &[u8]) -> Vec<(String, Option<String>)> {
    match node.kind() {
        "identifier" | "self" | "crate" | "super" => {
            let name = node_text(node, src);
            let full = if prefix.is_empty() { name.to_owned() } else { format!("{}::{}", prefix, name) };
            vec![(full, None)]
        }
        "scoped_identifier" => {
            vec![(node_text(node, src).to_owned(), None)]
        }
        "use_wildcard" => {
            let full = if prefix.is_empty() { "*".to_owned() } else { format!("{}::*", prefix) };
            vec![(full, None)]
        }
        "use_as_clause" => {
            let path = node
                .child_by_field_name("path")
                .map(|n| node_text(&n, src))
                .unwrap_or("");
            let alias = node
                .child_by_field_name("alias")
                .map(|n| node_text(&n, src).to_owned());
            let full = if prefix.is_empty() {
                path.to_owned()
            } else {
                format!("{}::{}", prefix, path)
            };
            vec![(full, alias)]
        }
        "use_list" => {
            let mut c = node.walk();
            node.children(&mut c)
                .filter(|n| n.is_named())
                .flat_map(|child| flatten_use(&child, prefix, src))
                .collect()
        }
        "scoped_use_list" => {
            let path = node
                .child_by_field_name("path")
                .map(|n| node_text(&n, src).to_owned())
                .unwrap_or_default();
            let new_prefix = if prefix.is_empty() {
                path
            } else if path.is_empty() {
                prefix.to_owned()
            } else {
                format!("{}::{}", prefix, path)
            };
            let list = node.child_by_field_name("list");
            if let Some(list) = list {
                flatten_use(&list, &new_prefix, src)
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

// ─── Context ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Ctx<'a> {
    src: &'a [u8],
    file: &'a str,
    scope: String,
    #[allow(dead_code)]
    enclosing_type: Option<NodeId>,
    #[allow(dead_code)]
    enclosing_fn: Option<NodeId>,
}

impl<'a> Ctx<'a> {
    fn new(src: &'a [u8], file: &'a str) -> Self {
        Self { src, file, scope: String::new(), enclosing_type: None, enclosing_fn: None }
    }
    fn child_scope(&self, name: &str) -> Self {
        Self { scope: qualify(&self.scope, name), ..self.clone() }
    }
}

// ─── Extractor ────────────────────────────────────────────────────────────────

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> Language { Language::Rust }

    fn extract(&self, source: &str, file: &str, graph: &mut DependencyGraph) {
        let src = source.as_bytes();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("failed to load Rust grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return,
        };

        let simple = std::path::Path::new(file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file);
        let mut file_node = Node::new(
            0, NodeKind::File, simple, file, file,
            Span::new(0, 0, 0, 0), Language::Rust,
        );
        file_node.visibility = Visibility::Public;
        file_node.hash = Some(hash_source(source));
        let file_id = graph.add_node(file_node);

        let ctx = Ctx::new(src, file);
        walk_items(&tree.root_node(), graph, &ctx, file_id);
    }
}

// ─── Item walker (handles attribute collection) ───────────────────────────────

fn walk_items(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mut pending_attrs: Vec<TsNode> = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        match child.kind() {
            "attribute_item" | "inner_attribute_item" => {
                pending_attrs.push(child);
            }
            "line_comment" | "block_comment" => {}
            _ => {
                let attrs = attribute_texts(&pending_attrs, ctx.src);
                pending_attrs.clear();
                dispatch_item(&child, graph, ctx, parent_id, attrs);
            }
        }
    }
}

fn dispatch_item(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    match node.kind() {
        "use_declaration"          => handle_use(node, graph, ctx, parent_id, attrs),
        "extern_crate_declaration" => handle_extern_crate(node, graph, ctx, parent_id, attrs),
        "mod_item"                 => handle_mod(node, graph, ctx, parent_id, attrs),
        "struct_item"              => handle_struct(node, graph, ctx, parent_id, attrs),
        "enum_item"                => handle_enum(node, graph, ctx, parent_id, attrs),
        "trait_item"               => handle_trait(node, graph, ctx, parent_id, attrs),
        "impl_item"                => handle_impl(node, graph, ctx, parent_id),
        "function_item"            => {
            handle_function(node, graph, ctx, parent_id, attrs, NodeKind::Function);
        }
        "const_item"               => handle_const(node, graph, ctx, parent_id, attrs),
        "static_item"              => handle_static(node, graph, ctx, parent_id, attrs),
        "type_item"                => handle_type_alias(node, graph, ctx, parent_id, attrs),
        "macro_invocation"         => {
            // Top-level macro (e.g. `println!` at module scope)
            // No enclosing function — skip body edges.
        }
        _ => {}
    }
}

// ─── use ──────────────────────────────────────────────────────────────────────

fn handle_use(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let vis = get_visibility(node, ctx.src);
    let is_pub = matches!(vis, Visibility::Public);

    let arg = match node.child_by_field_name("argument") {
        Some(a) => a,
        None => return,
    };
    let pairs = flatten_use(&arg, "", ctx.src);

    for (path, alias) in pairs {
        let simple = alias
            .as_deref()
            .or_else(|| path.rsplit("::").next())
            .unwrap_or(&path)
            .to_owned();

        let kind = if is_pub { NodeKind::Import } else { NodeKind::Import }; // both Import for now
        let mut imp = Node::new(
            0, kind, &simple, &path, ctx.file, ts_span(node), Language::Rust,
        );
        imp.visibility = vis.clone();
        imp.attributes = attrs.clone();
        imp.metadata.insert("wildcard".into(), path.ends_with('*').to_string());
        imp.metadata.insert("reexport".into(), is_pub.to_string());
        if let Some(ref a) = alias {
            imp.metadata.insert("alias".into(), a.clone());
        }
        let imp_id = graph.add_node(imp);

        let edge_kind = if is_pub { EdgeKind::Reexports } else { EdgeKind::Imports };
        graph.add_edge_simple(edge_kind, parent_id, EdgeTarget::Resolved(imp_id), ts_span(node));
        graph.add_edge_simple(EdgeKind::Imports, parent_id, EdgeTarget::Unresolved(path), ts_span(node));
    }
}

// ─── extern crate ─────────────────────────────────────────────────────────────

fn handle_extern_crate(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let alias = field_text(node, "alias", ctx.src).map(str::to_owned);
    let simple = alias.as_deref().unwrap_or(&name).to_owned();
    let mut imp = Node::new(
        0, NodeKind::Import, &simple, &name, ctx.file, ts_span(node), Language::Rust,
    );
    imp.visibility = get_visibility(node, ctx.src);
    imp.attributes = attrs;
    imp.metadata.insert("extern_crate".into(), "true".into());
    if let Some(a) = alias {
        imp.metadata.insert("alias".into(), a);
    }
    let imp_id = graph.add_node(imp);
    graph.add_edge_simple(EdgeKind::Imports, parent_id, EdgeTarget::Resolved(imp_id), ts_span(node));
}

// ─── mod ──────────────────────────────────────────────────────────────────────

fn handle_mod(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);

    let mut m = Node::new(0, NodeKind::Package, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    m.visibility = get_visibility(node, ctx.src);
    m.attributes = attrs;
    let mid = graph.add_node(m);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(mid), ts_span(node));

    if let Some(body) = node.child_by_field_name("body") {
        let child_ctx = ctx.child_scope(&name);
        walk_items(&body, graph, &child_ctx, mid);
    }
}

// ─── struct ───────────────────────────────────────────────────────────────────

fn handle_struct(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let tp = node.child_by_field_name("type_parameters");
    let wc = find_child(node, "where_clause");

    let mut s = Node::new(0, NodeKind::Class, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    s.visibility     = get_visibility(node, ctx.src);
    s.generic_params = collect_type_params(tp, ctx.src);
    s.generic_bounds = collect_type_bounds(tp, wc, ctx.src);
    s.attributes     = attrs;
    let sid = graph.add_node(s);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(sid), ts_span(node));

    // Named fields: field_declaration_list
    if let Some(body) = node.child_by_field_name("body") {
        let child_ctx = ctx.child_scope(&name);
        walk_struct_fields(&body, graph, &child_ctx, sid);
    }
}

fn walk_struct_fields(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    // Collect attrs for fields too
    let mut pending: Vec<TsNode> = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "attribute_item" => { pending.push(child); }
            "field_declaration" => {
                let attrs = attribute_texts(&pending, ctx.src);
                pending.clear();
                handle_field_decl(&child, graph, ctx, parent_id, attrs);
            }
            _ => { pending.clear(); }
        }
    }
}

fn handle_field_decl(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let type_text = field_text(node, "type", ctx.src).map(str::to_owned);
    let qname = qualify(&ctx.scope, &name);

    let mut f = Node::new(0, NodeKind::Field, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    f.visibility      = get_visibility(node, ctx.src);
    f.type_annotation = type_text.clone();
    f.attributes      = attrs;
    let fid = graph.add_node(f);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(fid), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::HasType, fid, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

// ─── enum ─────────────────────────────────────────────────────────────────────

fn handle_enum(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let tp = node.child_by_field_name("type_parameters");
    let wc = find_child(node, "where_clause");

    let mut e = Node::new(0, NodeKind::Enum, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    e.visibility     = get_visibility(node, ctx.src);
    e.generic_params = collect_type_params(tp, ctx.src);
    e.generic_bounds = collect_type_bounds(tp, wc, ctx.src);
    e.attributes     = attrs;
    let eid = graph.add_node(e);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(eid), ts_span(node));

    if let Some(body) = node.child_by_field_name("body") {
        let child_ctx = ctx.child_scope(&name);
        walk_enum_variants(&body, graph, &child_ctx, eid);
    }
}

fn walk_enum_variants(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mut pending: Vec<TsNode> = Vec::new();
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "attribute_item" => { pending.push(child); }
            "enum_variant" => {
                let attrs = attribute_texts(&pending, ctx.src);
                pending.clear();
                let name = field_text(&child, "name", ctx.src).unwrap_or("").to_owned();
                if name.is_empty() { continue; }
                let qname = qualify(&ctx.scope, &name);
                let mut v = Node::new(
                    0, NodeKind::Constant, &name, &qname, ctx.file, ts_span(&child), Language::Rust,
                );
                v.visibility = Visibility::Public;
                v.attributes = attrs;
                let vid = graph.add_node(v);
                graph.add_edge_simple(
                    EdgeKind::Contains, parent_id, EdgeTarget::Resolved(vid), ts_span(&child),
                );
            }
            _ => { pending.clear(); }
        }
    }
}

// ─── trait ────────────────────────────────────────────────────────────────────

fn handle_trait(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let tp = node.child_by_field_name("type_parameters");
    let wc = find_child(node, "where_clause");

    let mut t = Node::new(0, NodeKind::Interface, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    t.visibility     = get_visibility(node, ctx.src);
    t.generic_params = collect_type_params(tp, ctx.src);
    t.generic_bounds = collect_type_bounds(tp, wc, ctx.src);
    t.attributes     = attrs;

    // Supertraits from the `bounds` field
    if let Some(bounds) = node.child_by_field_name("bounds") {
        let mut c = bounds.walk();
        for b in bounds.children(&mut c) {
            if b.is_named() {
                let super_name = node_text(&b, ctx.src).to_owned();
                if !super_name.is_empty() {
                    // Supertraits become Extends edges (added after inserting the node).
                    t.generic_bounds.push(format!("Self: {}", super_name));
                }
            }
        }
    }

    let tid = graph.add_node(t);
    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(tid), ts_span(node));

    // Supertraits → Extends edges
    if let Some(bounds) = node.child_by_field_name("bounds") {
        let mut c = bounds.walk();
        for b in bounds.children(&mut c) {
            if b.is_named() {
                let super_name = node_text(&b, ctx.src).trim().to_owned();
                if !super_name.is_empty() {
                    graph.add_edge_simple(
                        EdgeKind::Extends, tid, EdgeTarget::Unresolved(super_name), ts_span(&b),
                    );
                }
            }
        }
    }

    // Methods in the trait body — use walk_impl_body so they get NodeKind::Method.
    if let Some(body) = node.child_by_field_name("body") {
        let child_ctx = Ctx { enclosing_type: Some(tid), ..ctx.child_scope(&name) };
        walk_impl_body(&body, graph, &child_ctx, tid);
    }
}

// ─── impl ─────────────────────────────────────────────────────────────────────

fn handle_impl(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, _parent_id: NodeId) {
    // The type being implemented for.
    let type_name = field_text(node, "type", ctx.src)
        .unwrap_or("")
        .split('<')   // strip generic args from the type name for lookup
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    if type_name.is_empty() { return; }

    // Try to resolve the struct/enum node.
    let qname = qualify(&ctx.scope, &type_name);
    let type_id = graph
        .find_by_qualified(&qname)
        .or_else(|| graph.find_by_qualified(&type_name));

    // If implementing a trait → Implements edge.
    if let Some(trait_node) = node.child_by_field_name("trait") {
        let trait_name = node_text(&trait_node, ctx.src)
            .split('<')
            .next()
            .unwrap_or("")
            .trim()
            .to_owned();
        if !trait_name.is_empty() {
            if let Some(tid) = type_id {
                graph.add_edge_simple(
                    EdgeKind::Implements, tid, EdgeTarget::Unresolved(trait_name), ts_span(node),
                );
            }
        }
    }

    // Walk the impl body — methods are Methods, consts are Constants.
    if let Some(body) = node.child_by_field_name("body") {
        // Use a child scope keyed by the type so methods get qualified names.
        let child_ctx = Ctx {
            enclosing_type: type_id,
            ..ctx.child_scope(&type_name)
        };
        let parent = type_id.unwrap_or(_parent_id);
        walk_impl_body(&body, graph, &child_ctx, parent);
    }
}

fn walk_impl_body(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mut pending: Vec<TsNode> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "attribute_item" => { pending.push(child); }
            "function_item" | "function_signature_item" => {
                let attrs = attribute_texts(&pending, ctx.src);
                pending.clear();
                handle_function(&child, graph, ctx, parent_id, attrs, NodeKind::Method);
            }
            "const_item" => {
                let attrs = attribute_texts(&pending, ctx.src);
                pending.clear();
                handle_const(&child, graph, ctx, parent_id, attrs);
            }
            "type_item" => {
                let attrs = attribute_texts(&pending, ctx.src);
                pending.clear();
                handle_type_alias(&child, graph, ctx, parent_id, attrs);
            }
            _ => { pending.clear(); }
        }
    }
}

// ─── function / method ────────────────────────────────────────────────────────

fn handle_function(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
    kind: NodeKind,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let tp = node.child_by_field_name("type_parameters");
    let wc = find_child(node, "where_clause");

    let mut f = Node::new(0, kind, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    f.visibility      = get_visibility(node, ctx.src);
    f.is_async        = is_async(node, ctx.src);
    f.is_abstract     = node.child_by_field_name("body").is_none(); // trait method without body
    f.generic_params  = collect_type_params(tp, ctx.src);
    f.generic_bounds  = collect_type_bounds(tp, wc, ctx.src);
    f.type_annotation = field_text(node, "return_type", ctx.src).map(str::to_owned);
    f.attributes      = attrs;
    let fid = graph.add_node(f);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(fid), ts_span(node));

    if let Some(ret) = field_text(node, "return_type", ctx.src) {
        // strip leading `->` whitespace that tree-sitter may include
        let ret = ret.trim_start_matches("->").trim();
        if !ret.is_empty() && ret != "()" {
            graph.add_edge_simple(
                EdgeKind::Returns, fid, EdgeTarget::Unresolved(ret.to_owned()), ts_span(node),
            );
        }
    }

    // Parameters
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_parameters(&params, graph, ctx, fid);
    }

    // Body
    if let Some(body) = node.child_by_field_name("body") {
        let child_ctx = Ctx { enclosing_fn: Some(fid), ..ctx.child_scope(&name) };
        walk_fn_body(&body, graph, &child_ctx, fid);
    }
}

fn handle_parameters(params_node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = params_node.walk();
    for param in params_node.children(&mut cursor) {
        match param.kind() {
            "self_parameter" => {
                // &self / &mut self / self — not a separate Parameter node, skip.
            }
            "parameter" => {
                // pattern field gives the name; type field gives the type.
                let pat = param.child_by_field_name("pattern");
                let name = pat
                    .map(|p| node_text(&p, ctx.src))
                    .unwrap_or("")
                    .trim_start_matches("mut ")
                    .to_owned();
                if name.is_empty() || name == "self" { continue; }
                let type_text = field_text(&param, "type", ctx.src).map(str::to_owned);
                let qname = qualify(&ctx.scope, &name);

                let mut p = Node::new(
                    0, NodeKind::Parameter, &name, &qname,
                    ctx.file, ts_span(&param), Language::Rust,
                );
                p.type_annotation = type_text.clone();
                let pid = graph.add_node(p);

                graph.add_edge_simple(
                    EdgeKind::HasParameter, fn_id, EdgeTarget::Resolved(pid), ts_span(&param),
                );
                if let Some(t) = type_text {
                    graph.add_edge_simple(
                        EdgeKind::HasType, pid, EdgeTarget::Unresolved(t), ts_span(&param),
                    );
                }
            }
            _ => {}
        }
    }
}

// ─── Function body ────────────────────────────────────────────────────────────

fn walk_fn_body(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_fn_expr(&child, graph, ctx, fn_id);
    }
}

fn dispatch_fn_expr(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    match node.kind() {
        "let_declaration"          => handle_let(node, graph, ctx, fn_id),
        "assignment_expression"
        | "compound_assignment_expr" => handle_assignment(node, graph, ctx, fn_id),
        "call_expression"          => handle_call(node, graph, ctx, fn_id),
        "macro_invocation"         => handle_macro(node, graph, ctx, fn_id),
        "struct_expression"        => handle_struct_expr(node, graph, ctx, fn_id),
        "closure_expression"       => handle_closure(node, graph, ctx, fn_id),
        "await_expression"         => handle_await(node, graph, ctx, fn_id),
        "return_expression" => {
            // Recurse into the returned expression.
            let mut c = node.walk();
            for child in node.children(&mut c) {
                if child.is_named() {
                    dispatch_fn_expr(&child, graph, ctx, fn_id);
                }
            }
        }
        "expression_statement" => {
            if let Some(inner) = node.named_child(0) {
                dispatch_fn_expr(&inner, graph, ctx, fn_id);
            }
        }
        // Container nodes — recurse into all children.
        "block"
        | "if_expression"
        | "let_condition"
        | "else_clause"
        | "match_expression"
        | "match_arm"
        | "match_block"
        | "for_expression"
        | "while_expression"
        | "loop_expression"
        | "unsafe_block"
        | "async_block"
        | "try_expression"
        | "tuple_expression"
        | "array_expression"
        | "parenthesized_expression"
        | "binary_expression"
        | "unary_expression"
        | "reference_expression"
        | "type_cast_expression"
        | "field_expression"
        | "index_expression"
        | "range_expression"
        | "scoped_expression"
        | "arguments" => {
            walk_fn_body(node, graph, ctx, fn_id);
        }
        // Unknown named nodes — recurse so we don't silently drop subtrees.
        _ if node.is_named() && node.child_count() > 0 => {
            walk_fn_body(node, graph, ctx, fn_id);
        }
        _ => {}
    }
}

// ─── let ──────────────────────────────────────────────────────────────────────

fn handle_let(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let pat = node.child_by_field_name("pattern");
    let name = pat
        .map(|p| node_text(&p, ctx.src))
        .unwrap_or("")
        .trim_start_matches("mut ")
        .to_owned();
    if name.is_empty() { return; }

    let type_text = field_text(node, "type", ctx.src).map(str::to_owned);
    let qname = qualify(&ctx.scope, &name);

    let mut v = Node::new(0, NodeKind::Variable, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    v.type_annotation = type_text.clone();
    let vid = graph.add_node(v);

    graph.add_edge_simple(EdgeKind::Contains, fn_id, EdgeTarget::Resolved(vid), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::HasType, vid, EdgeTarget::Unresolved(t), ts_span(node));
    }
    if let Some(val) = node.child_by_field_name("value") {
        graph.add_edge_simple(EdgeKind::Writes, fn_id, EdgeTarget::Resolved(vid), ts_span(node));
        dispatch_fn_expr(&val, graph, ctx, fn_id);
    }
}

// ─── assignment ───────────────────────────────────────────────────────────────

fn handle_assignment(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    if let Some(lhs) = node.child_by_field_name("left") {
        let raw = node_text(&lhs, ctx.src).trim().to_owned();
        // Extract base name: `self.field` → `field`, `x` → `x`
        let base = raw
            .trim_start_matches("self.")
            .split('.')
            .last()
            .unwrap_or(&raw)
            .trim()
            .to_owned();
        if !base.is_empty() && base != "self" {
            graph.add_edge_simple(
                EdgeKind::Writes, fn_id, EdgeTarget::Unresolved(base), ts_span(&lhs),
            );
        }
    }
    if let Some(rhs) = node.child_by_field_name("right") {
        dispatch_fn_expr(&rhs, graph, ctx, fn_id);
    }
}

// ─── call expression ──────────────────────────────────────────────────────────

fn handle_call(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let func = match node.child_by_field_name("function") {
        Some(f) => f,
        None => return,
    };

    // Method call: function is a field_expression (receiver.method)
    let (callee, receiver) = if func.kind() == "field_expression" {
        let method_name = field_text(&func, "field", ctx.src).unwrap_or("").to_owned();
        let recv_node = func.child_by_field_name("value");
        let recv = recv_node.as_ref().map(|r| node_text(r, ctx.src).to_owned());
        (method_name, recv)
    } else {
        // Regular call: function is an identifier or scoped_identifier
        let callee = node_text(&func, ctx.src)
            .rsplit("::")
            .next()
            .unwrap_or("")
            .to_owned();
        (callee, None)
    };

    if !callee.is_empty() {
        let arity = node
            .child_by_field_name("arguments")
            .map(|args| {
                let mut c = args.walk();
                args.children(&mut c).filter(|n| n.is_named()).count() as u32
            })
            .unwrap_or(0);

        let mut edge = Edge::new(
            0, EdgeKind::Calls, fn_id, EdgeTarget::Unresolved(callee), ts_span(node),
        );
        edge.call_arity = Some(arity);
        graph.add_edge(edge);

        // Receiver field read
        if let Some(recv) = receiver {
            let base = recv
                .trim_start_matches("self.")
                .split('.')
                .last()
                .unwrap_or(&recv)
                .trim()
                .to_owned();
            if !base.is_empty() && !matches!(base.as_str(), "self" | "Self") {
                graph.add_edge_simple(
                    EdgeKind::Reads, fn_id, EdgeTarget::Unresolved(base), ts_span(&func),
                );
            }
        }
    }

    // Recurse into arguments
    if let Some(args) = node.child_by_field_name("arguments") {
        walk_fn_body(&args, graph, ctx, fn_id);
    }
    // Recurse into function expression (may be a field_expression with a nested call)
    walk_fn_body(&func, graph, ctx, fn_id);
}

// ─── macro invocation ─────────────────────────────────────────────────────────

fn handle_macro(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let macro_name = node
        .child_by_field_name("macro")
        .map(|n| node_text(&n, ctx.src).to_owned())
        .unwrap_or_default();
    if macro_name.is_empty() { return; }

    let mut edge = Edge::new(
        0, EdgeKind::Calls, fn_id, EdgeTarget::Unresolved(macro_name + "!"), ts_span(node),
    );
    edge.call_arity = Some(0); // arity unknown for macros
    graph.add_edge(edge);
}

// ─── struct expression (instantiation) ───────────────────────────────────────

fn handle_struct_expr(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let type_name = node_text(&name_node, ctx.src)
            .split('<')
            .next()
            .unwrap_or("")
            .rsplit("::")
            .next()
            .unwrap_or("")
            .trim()
            .to_owned();
        if !type_name.is_empty() {
            graph.add_edge_simple(
                EdgeKind::Instantiates, fn_id, EdgeTarget::Unresolved(type_name), ts_span(&name_node),
            );
        }
    }
}

// ─── closure ──────────────────────────────────────────────────────────────────

fn handle_closure(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let qname = qualify(&ctx.scope, "<closure>");
    let mut cl = Node::new(
        0, NodeKind::Closure, "<closure>", &qname, ctx.file, ts_span(node), Language::Rust,
    );
    cl.metadata.insert("enclosing_fn".into(), fn_id.to_string());
    let cid = graph.add_node(cl);

    graph.add_edge_simple(EdgeKind::Contains, fn_id, EdgeTarget::Resolved(cid), ts_span(node));

    if let Some(body) = node.child_by_field_name("body") {
        let child_ctx = Ctx { enclosing_fn: Some(cid), ..ctx.child_scope("<closure>") };
        dispatch_fn_expr(&body, graph, &child_ctx, cid);
    }
}

// ─── await ────────────────────────────────────────────────────────────────────

fn handle_await(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    // The awaited expression is the first named child.
    if let Some(inner) = node.named_child(0) {
        // If it's a call, record an Awaits edge to the callee name.
        if inner.kind() == "call_expression" {
            if let Some(func) = inner.child_by_field_name("function") {
                let callee = if func.kind() == "field_expression" {
                    field_text(&func, "field", ctx.src).unwrap_or("").to_owned()
                } else {
                    node_text(&func, ctx.src).rsplit("::").next().unwrap_or("").to_owned()
                };
                if !callee.is_empty() {
                    graph.add_edge_simple(
                        EdgeKind::Awaits, fn_id, EdgeTarget::Unresolved(callee), ts_span(node),
                    );
                }
            }
        }
        dispatch_fn_expr(&inner, graph, ctx, fn_id);
    }
}

// ─── const / static / type alias ─────────────────────────────────────────────

fn handle_const(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let type_text = field_text(node, "type", ctx.src).map(str::to_owned);

    let mut c = Node::new(0, NodeKind::Constant, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    c.visibility      = get_visibility(node, ctx.src);
    c.type_annotation = type_text.clone();
    c.attributes      = attrs;
    let cid = graph.add_node(c);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(cid), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::HasType, cid, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

fn handle_static(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let type_text = field_text(node, "type", ctx.src).map(str::to_owned);
    let is_mut = find_child(node, "mutable_specifier").is_some();

    let kind = if is_mut { NodeKind::StaticField } else { NodeKind::Constant };
    let mut s = Node::new(0, kind, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    s.visibility      = get_visibility(node, ctx.src);
    s.type_annotation = type_text.clone();
    s.attributes      = attrs;
    if is_mut { s.metadata.insert("mutable".into(), "true".into()); }
    let sid = graph.add_node(s);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(sid), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::HasType, sid, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

fn handle_type_alias(
    node: &TsNode,
    graph: &mut DependencyGraph,
    ctx: &Ctx,
    parent_id: NodeId,
    attrs: Vec<String>,
) {
    let name = field_text(node, "name", ctx.src).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let type_text = field_text(node, "type", ctx.src).map(str::to_owned);
    let tp = node.child_by_field_name("type_parameters");

    let mut a = Node::new(0, NodeKind::TypeAlias, &name, &qname, ctx.file, ts_span(node), Language::Rust);
    a.visibility      = get_visibility(node, ctx.src);
    a.generic_params  = collect_type_params(tp, ctx.src);
    a.type_annotation = type_text.clone();
    a.attributes      = attrs;
    let aid = graph.add_node(a);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(aid), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::References, aid, EdgeTarget::Unresolved(t), ts_span(node));
    }
}
