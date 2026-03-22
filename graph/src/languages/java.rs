use tree_sitter::{Node as TsNode, Parser};

use crate::graph::{
    DependencyGraph, Edge, EdgeKind, EdgeTarget, Language, Node, NodeId, NodeKind, Span, Visibility,
};
use crate::parser::{hash_source, LanguageExtractor};

// ─── Low-level tree-sitter helpers ────────────────────────────────────────────

fn ts_span(node: &TsNode) -> Span {
    let s = node.start_position();
    let e = node.end_position();
    Span::new(s.row as u32, s.column as u32, e.row as u32, e.column as u32)
}

fn node_text<'a>(node: &TsNode, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// First named field value as text.
fn field_text<'a>(parent: &TsNode, field: &str, source: &'a [u8]) -> Option<&'a str> {
    parent
        .child_by_field_name(field)
        .map(|n| node_text(&n, source))
}

/// First direct child whose `kind()` matches.
fn find_child<'a>(parent: &'a TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    let mut c = parent.walk();
    parent.children(&mut c).find(|n| n.kind() == kind)
}

/// All direct children whose `kind()` matches.
fn children_of_kind<'a>(parent: &'a TsNode<'a>, kind: &str) -> Vec<TsNode<'a>> {
    let mut c = parent.walk();
    parent.children(&mut c).filter(|n| n.kind() == kind).collect()
}

// ─── Modifiers (child node, never a named field in Java grammar) ──────────────

fn get_modifiers<'a>(parent: &'a TsNode<'a>) -> Option<TsNode<'a>> {
    find_child(parent, "modifiers")
}

fn visibility_from_modifiers(mods: Option<TsNode>, source: &[u8]) -> Visibility {
    let Some(m) = mods else { return Visibility::PackagePrivate };
    let text = node_text(&m, source);
    if text.contains("public")    { Visibility::Public }
    else if text.contains("protected") { Visibility::Protected }
    else if text.contains("private")   { Visibility::Private }
    else                               { Visibility::PackagePrivate }
}

fn has_modifier(mods: Option<TsNode>, modifier: &str, source: &[u8]) -> bool {
    mods.map(|m| node_text(&m, source).contains(modifier)).unwrap_or(false)
}

/// Collect `@Annotation` / `@marker_annotation` text from a modifiers node.
fn collect_annotations(mods: Option<TsNode>, source: &[u8]) -> Vec<String> {
    let Some(m) = mods else { return vec![] };
    let mut c = m.walk();
    m.children(&mut c)
        .filter(|n| n.kind() == "annotation" || n.kind() == "marker_annotation")
        .map(|n| node_text(&n, source).to_owned())
        .collect()
}

// ─── Generics ─────────────────────────────────────────────────────────────────

/// Walk `type_parameters` → each `type_parameter` → first `type_identifier`.
fn collect_type_params(tp_node: Option<TsNode>, source: &[u8]) -> Vec<String> {
    let Some(tp) = tp_node else { return vec![] };
    children_of_kind(&tp, "type_parameter")
        .iter()
        .filter_map(|p| {
            // type_parameter children: type_identifier, optional type_bound
            let mut c = p.walk();
            p.children(&mut c)
                .find(|n| n.kind() == "type_identifier")
                .map(|n| node_text(&n, source).to_owned())
        })
        .collect()
}

/// Raw bound strings for type params that have bounds.
fn collect_type_bounds(tp_node: Option<TsNode>, source: &[u8]) -> Vec<String> {
    let Some(tp) = tp_node else { return vec![] };
    children_of_kind(&tp, "type_parameter")
        .iter()
        .filter_map(|p| {
            if find_child(p, "type_bound").is_some() {
                Some(node_text(p, source).to_owned())
            } else {
                None
            }
        })
        .collect()
}

// ─── Throws (child node, not a named field) ───────────────────────────────────

/// Extract exception type names from a `throws` child node.
fn collect_throws(parent: &TsNode, source: &[u8]) -> Vec<String> {
    let Some(throws) = find_child(parent, "throws") else { return vec![] };
    // throws children are _type nodes (type_identifier, scoped_type_identifier, …)
    let mut c = throws.walk();
    throws
        .children(&mut c)
        .filter(|n| n.is_named() && n.kind() != "throws") // skip punctuation
        .map(|n| node_text(&n, source).to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

// ─── Superclass / interfaces ───────────────────────────────────────────────────

/// `superclass` node child → plain type text (strips "extends" keyword).
fn superclass_name(cls: &TsNode, source: &[u8]) -> Option<String> {
    let sc = find_child(cls, "superclass")?;
    // superclass children: _type — take first named child
    let mut c = sc.walk();
    sc.children(&mut c)
        .find(|n| n.is_named())
        .map(|n| node_text(&n, source).to_owned())
}

/// `super_interfaces` / `interfaces` field → list of interface names.
fn interface_names(node: &TsNode, source: &[u8]) -> Vec<String> {
    // class/enum use field "interfaces" → super_interfaces node
    // interface uses child "extends_interfaces" → extends_interfaces node
    let si = node
        .child_by_field_name("interfaces")
        .or_else(|| find_child(node, "extends_interfaces"));
    let Some(si) = si else { return vec![] };

    // super_interfaces / extends_interfaces contains a type_list whose children are _type
    let type_list = find_child(&si, "type_list").unwrap_or(si);
    let mut c = type_list.walk();
    type_list
        .children(&mut c)
        .filter(|n| n.is_named())
        .map(|n| node_text(&n, source).trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

// ─── Qualified names ──────────────────────────────────────────────────────────

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() { name.to_owned() } else { format!("{}.{}", scope, name) }
}

// ─── Context ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Ctx<'a> {
    source: &'a [u8],
    file: &'a str,
    scope: String,
    enclosing_type: Option<NodeId>,
    #[allow(dead_code)]
    enclosing_method: Option<NodeId>,
}

impl<'a> Ctx<'a> {
    fn new(source: &'a [u8], file: &'a str) -> Self {
        Self { source, file, scope: String::new(), enclosing_type: None, enclosing_method: None }
    }
    fn child_scope(&self, name: &str) -> Self {
        Self { scope: qualify(&self.scope, name), ..self.clone() }
    }
}

// ─── Extractor ────────────────────────────────────────────────────────────────

pub struct JavaExtractor;

impl LanguageExtractor for JavaExtractor {
    fn language(&self) -> Language { Language::Java }

    fn extract(&self, source: &str, file: &str, graph: &mut DependencyGraph) {
        let src = source.as_bytes();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .expect("failed to load Java grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return,
        };

        let simple_name = std::path::Path::new(file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file);
        let mut file_node = Node::new(
            0, NodeKind::File, simple_name, file, file,
            Span::new(0, 0, 0, 0), Language::Java,
        );
        file_node.visibility = Visibility::Public;
        file_node.hash = Some(hash_source(source));
        let file_id = graph.add_node(file_node);

        let ctx = Ctx::new(src, file);
        walk_program(&tree.root_node(), graph, &ctx, file_id);
    }
}

// ─── Top-level ────────────────────────────────────────────────────────────────

fn walk_program(root: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "package_declaration"        => handle_package(&child, graph, ctx, file_id),
            "import_declaration"         => handle_import(&child, graph, ctx, file_id),
            "class_declaration"          => handle_class(&child, graph, ctx, file_id),
            "interface_declaration"      => handle_interface(&child, graph, ctx, file_id),
            "enum_declaration"           => handle_enum(&child, graph, ctx, file_id),
            "annotation_type_declaration"=> handle_annotation_type(&child, graph, ctx, file_id),
            _ => {}
        }
    }
}

// ─── Package ──────────────────────────────────────────────────────────────────

fn handle_package(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut c = node.walk();
    let pkg_name = node
        .children(&mut c)
        .find(|n| matches!(n.kind(), "scoped_identifier" | "identifier"))
        .map(|n| node_text(&n, ctx.source).to_owned())
        .unwrap_or_default();
    if pkg_name.is_empty() { return; }

    let mut pkg_node = Node::new(
        0, NodeKind::Package, &pkg_name, &pkg_name,
        ctx.file, ts_span(node), Language::Java,
    );
    pkg_node.visibility = Visibility::Public;
    let pkg_id = graph.add_node(pkg_node);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(pkg_id), ts_span(node));
}

// ─── Import ───────────────────────────────────────────────────────────────────

fn handle_import(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let raw = node_text(node, ctx.source);
    let cleaned = raw
        .trim_start_matches("import")
        .trim_start_matches("static")
        .trim()
        .trim_end_matches(';')
        .trim()
        .to_owned();
    if cleaned.is_empty() { return; }

    let simple = cleaned.rsplit('.').next().unwrap_or(&cleaned).to_owned();
    let mut imp = Node::new(
        0, NodeKind::Import, &simple, &cleaned,
        ctx.file, ts_span(node), Language::Java,
    );
    imp.metadata.insert("static".into(),   raw.contains("static").to_string());
    imp.metadata.insert("wildcard".into(), cleaned.ends_with('*').to_string());
    let imp_id = graph.add_node(imp);

    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(node));
    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Unresolved(cleaned), ts_span(node));
}

// ─── Annotation type ──────────────────────────────────────────────────────────

fn handle_annotation_type(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, _parent_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let mods = get_modifiers(node);
    let mut n = Node::new(
        0, NodeKind::Annotation, &name, &qualify(&ctx.scope, &name),
        ctx.file, ts_span(node), Language::Java,
    );
    n.visibility  = visibility_from_modifiers(mods, ctx.source);
    n.attributes  = collect_annotations(mods, ctx.source);
    graph.add_node(n);
}

// ─── Class ────────────────────────────────────────────────────────────────────

fn handle_class(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let mods   = get_modifiers(node);
    let tp     = node.child_by_field_name("type_parameters");

    let mut cls = Node::new(0, NodeKind::Class, &name, &qname, ctx.file, ts_span(node), Language::Java);
    cls.visibility      = visibility_from_modifiers(mods, ctx.source);
    cls.is_abstract     = has_modifier(mods, "abstract", ctx.source);
    cls.generic_params  = collect_type_params(tp, ctx.source);
    cls.generic_bounds  = collect_type_bounds(tp, ctx.source);
    cls.attributes      = collect_annotations(mods, ctx.source);
    let cls_id = graph.add_node(cls);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(cls_id), ts_span(node));

    // Extends
    if let Some(super_name) = superclass_name(node, ctx.source) {
        graph.add_edge_simple(
            EdgeKind::Extends, cls_id, EdgeTarget::Unresolved(super_name), ts_span(node),
        );
    }

    // Implements
    for iface in interface_names(node, ctx.source) {
        graph.add_edge_simple(
            EdgeKind::Implements, cls_id, EdgeTarget::Unresolved(iface), ts_span(node),
        );
    }

    let child_ctx = Ctx { enclosing_type: Some(cls_id), ..ctx.child_scope(&name) };
    if let Some(body) = node.child_by_field_name("body") {
        walk_class_body(&body, graph, &child_ctx, cls_id);
    }
}

// ─── Interface ────────────────────────────────────────────────────────────────

fn handle_interface(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let mods   = get_modifiers(node);
    let tp     = node.child_by_field_name("type_parameters");

    let mut iface = Node::new(0, NodeKind::Interface, &name, &qname, ctx.file, ts_span(node), Language::Java);
    iface.visibility     = visibility_from_modifiers(mods, ctx.source);
    iface.generic_params = collect_type_params(tp, ctx.source);
    iface.generic_bounds = collect_type_bounds(tp, ctx.source);
    iface.attributes     = collect_annotations(mods, ctx.source);
    let iface_id = graph.add_node(iface);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(iface_id), ts_span(node));

    for parent_iface in interface_names(node, ctx.source) {
        graph.add_edge_simple(
            EdgeKind::Extends, iface_id, EdgeTarget::Unresolved(parent_iface), ts_span(node),
        );
    }

    let child_ctx = Ctx { enclosing_type: Some(iface_id), ..ctx.child_scope(&name) };
    if let Some(body) = node.child_by_field_name("body") {
        walk_class_body(&body, graph, &child_ctx, iface_id);
    }
}

// ─── Enum ─────────────────────────────────────────────────────────────────────

fn handle_enum(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let mods   = get_modifiers(node);

    let mut en = Node::new(0, NodeKind::Enum, &name, &qname, ctx.file, ts_span(node), Language::Java);
    en.visibility = visibility_from_modifiers(mods, ctx.source);
    en.attributes = collect_annotations(mods, ctx.source);
    let enum_id = graph.add_node(en);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(enum_id), ts_span(node));

    for iface in interface_names(node, ctx.source) {
        graph.add_edge_simple(
            EdgeKind::Implements, enum_id, EdgeTarget::Unresolved(iface), ts_span(node),
        );
    }

    let child_ctx = Ctx { enclosing_type: Some(enum_id), ..ctx.child_scope(&name) };
    if let Some(body) = node.child_by_field_name("body") {
        walk_enum_body(&body, graph, &child_ctx, enum_id);
        walk_class_body(&body, graph, &child_ctx, enum_id);
    }
}

fn walk_enum_body(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    for constant in children_of_kind(body, "enum_constant") {
        let name = field_text(&constant, "name", ctx.source).unwrap_or("").to_owned();
        if name.is_empty() { continue; }
        let qname = qualify(&ctx.scope, &name);
        let mods  = get_modifiers(&constant);
        let mut cn = Node::new(0, NodeKind::Constant, &name, &qname, ctx.file, ts_span(&constant), Language::Java);
        cn.visibility = Visibility::Public;
        cn.attributes = collect_annotations(mods, ctx.source);
        let cid = graph.add_node(cn);
        graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(cid), ts_span(&constant));
    }
}

// ─── Class body ───────────────────────────────────────────────────────────────

fn walk_class_body(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "field_declaration"          => handle_field(&child, graph, ctx, parent_id),
            "method_declaration"
            | "constructor_declaration"  => handle_method(&child, graph, ctx, parent_id),
            "class_declaration"          => handle_class(&child, graph, ctx, parent_id),
            "interface_declaration"      => handle_interface(&child, graph, ctx, parent_id),
            "enum_declaration"           => handle_enum(&child, graph, ctx, parent_id),
            "annotation_type_declaration"=> handle_annotation_type(&child, graph, ctx, parent_id),
            _ => {}
        }
    }
}

// ─── Field ────────────────────────────────────────────────────────────────────

fn handle_field(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let mods       = get_modifiers(node);
    let is_static  = has_modifier(mods, "static", ctx.source);
    let is_final   = has_modifier(mods, "final",  ctx.source);
    let type_text  = field_text(node, "type", ctx.source).map(str::to_owned);
    let visibility = visibility_from_modifiers(mods, ctx.source);
    let annotations = collect_annotations(mods, ctx.source);

    // A single declaration can have multiple declarators: `int x, y;`
    let mut cursor = node.walk();
    for decl in node.children_by_field_name("declarator", &mut cursor) {
        let name = field_text(&decl, "name", ctx.source).unwrap_or("").to_owned();
        if name.is_empty() { continue; }
        let qname = qualify(&ctx.scope, &name);

        let kind = if is_static && is_final { NodeKind::Constant }
                   else if is_static        { NodeKind::StaticField }
                   else                     { NodeKind::Field };

        let mut f = Node::new(0, kind, &name, &qname, ctx.file, ts_span(&decl), Language::Java);
        f.visibility      = visibility.clone();
        f.type_annotation = type_text.clone();
        f.attributes      = annotations.clone();
        let fid = graph.add_node(f);

        graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(fid), ts_span(&decl));

        if let Some(ref t) = type_text {
            graph.add_edge_simple(EdgeKind::HasType, fid, EdgeTarget::Unresolved(t.clone()), ts_span(&decl));
        }
    }
}

// ─── Method / Constructor ─────────────────────────────────────────────────────

fn handle_method(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, parent_id: NodeId) {
    let is_constructor = node.kind() == "constructor_declaration";
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let mods   = get_modifiers(node);
    let tp     = node.child_by_field_name("type_parameters");

    let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(node), Language::Java);
    m.visibility      = visibility_from_modifiers(mods, ctx.source);
    m.is_abstract     = has_modifier(mods, "abstract", ctx.source)
                        || (!is_constructor && node.child_by_field_name("body").is_none());
    m.is_constructor  = is_constructor;
    m.generic_params  = collect_type_params(tp, ctx.source);
    m.generic_bounds  = collect_type_bounds(tp, ctx.source);
    m.attributes      = collect_annotations(mods, ctx.source);
    m.type_annotation = field_text(node, "type", ctx.source).map(str::to_owned);
    if has_modifier(mods, "static", ctx.source) {
        m.metadata.insert("static".into(), "true".into());
    }
    let mid = graph.add_node(m);

    graph.add_edge_simple(EdgeKind::Contains, parent_id, EdgeTarget::Resolved(mid), ts_span(node));

    // Return type.
    if !is_constructor {
        if let Some(ret) = field_text(node, "type", ctx.source) {
            if ret != "void" {
                graph.add_edge_simple(EdgeKind::Returns, mid, EdgeTarget::Unresolved(ret.to_owned()), ts_span(node));
            }
        }
    }

    // Throws (child node, not a named field).
    for exc in collect_throws(node, ctx.source) {
        graph.add_edge_simple(EdgeKind::Throws, mid, EdgeTarget::Unresolved(exc), ts_span(node));
    }

    // Parameters.
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_parameters(&params, graph, ctx, mid);
    }

    // Body.
    let child_ctx = Ctx { enclosing_method: Some(mid), ..ctx.child_scope(&name) };
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, &child_ctx, mid);
    }
}

// ─── Parameters ───────────────────────────────────────────────────────────────

fn handle_parameters(params_node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    let mut cursor = params_node.walk();
    for param in params_node.children(&mut cursor) {
        if !matches!(param.kind(), "formal_parameter" | "spread_parameter") { continue; }
        let name      = field_text(&param, "name", ctx.source).unwrap_or("").to_owned();
        let type_text = field_text(&param, "type", ctx.source).map(str::to_owned);
        if name.is_empty() { continue; }

        let mut p = Node::new(
            0, NodeKind::Parameter, &name, &qualify(&ctx.scope, &name),
            ctx.file, ts_span(&param), Language::Java,
        );
        p.type_annotation = type_text.clone();
        let pid = graph.add_node(p);

        graph.add_edge_simple(EdgeKind::HasParameter, method_id, EdgeTarget::Resolved(pid), ts_span(&param));
        if let Some(t) = type_text {
            graph.add_edge_simple(EdgeKind::HasType, pid, EdgeTarget::Unresolved(t), ts_span(&param));
        }
    }
}

// ─── Method body ──────────────────────────────────────────────────────────────

/// Dispatch a single expression/statement node. Unlike `walk_body` (which
/// iterates a container's children), this handles the node itself.
fn dispatch_expr(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    match node.kind() {
        "local_variable_declaration" => handle_local_var(node, graph, ctx, method_id),
        "assignment_expression"      => handle_assignment(node, graph, ctx, method_id),
        "method_invocation"
        | "explicit_generic_invocation" => handle_call(node, graph, ctx, method_id),
        "object_creation_expression" => handle_instantiation(node, graph, ctx, method_id),
        "lambda_expression"          => handle_lambda(node, graph, ctx, method_id),
        "expression_statement" => {
            // Use named_child(0) to skip punctuation/whitespace unnamed nodes.
            if let Some(expr) = node.named_child(0) {
                dispatch_expr(&expr, graph, ctx, method_id);
            }
        }
        _ => walk_body(node, graph, ctx, method_id),
    }
}

fn walk_body(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_expr(&child, graph, ctx, method_id);
    }
}

// ─── Local variable ───────────────────────────────────────────────────────────

fn handle_local_var(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    let type_text = field_text(node, "type", ctx.source).map(str::to_owned);
    let mut cursor = node.walk();
    for decl in node.children_by_field_name("declarator", &mut cursor) {
        let name = field_text(&decl, "name", ctx.source).unwrap_or("").to_owned();
        if name.is_empty() { continue; }

        let mut v = Node::new(
            0, NodeKind::Variable, &name, &qualify(&ctx.scope, &name),
            ctx.file, ts_span(&decl), Language::Java,
        );
        v.type_annotation = type_text.clone();
        let vid = graph.add_node(v);

        graph.add_edge_simple(EdgeKind::Contains, method_id, EdgeTarget::Resolved(vid), ts_span(&decl));
        if let Some(ref t) = type_text {
            graph.add_edge_simple(EdgeKind::HasType, vid, EdgeTarget::Unresolved(t.clone()), ts_span(&decl));
        }
        if let Some(val) = decl.child_by_field_name("value") {
            graph.add_edge_simple(EdgeKind::Writes, method_id, EdgeTarget::Resolved(vid), ts_span(&decl));
            dispatch_expr(&val, graph, ctx, method_id);
        }
    }
}

// ─── Assignment ───────────────────────────────────────────────────────────────

fn handle_assignment(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    if let Some(lhs) = node.child_by_field_name("left") {
        let raw = node_text(&lhs, ctx.source);
        let base = raw
            .trim_start_matches("this.")
            .split('.')
            .last()
            .unwrap_or(raw)
            .trim()
            .to_owned();
        if !base.is_empty() {
            graph.add_edge_simple(EdgeKind::Writes, method_id, EdgeTarget::Unresolved(base), ts_span(&lhs));
        }
    }
    if let Some(rhs) = node.child_by_field_name("right") {
        walk_body(&rhs, graph, ctx, method_id);
    }
}

// ─── Method call ──────────────────────────────────────────────────────────────

fn handle_call(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    let callee = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if callee.is_empty() { return; }

    let arity = node.child_by_field_name("arguments").map(|args| {
        let mut c = args.walk();
        args.children(&mut c)
            .filter(|n| n.is_named())
            .count() as u32
    }).unwrap_or(0);

    let mut edge = Edge::new(0, EdgeKind::Calls, method_id, EdgeTarget::Unresolved(callee), ts_span(node));
    edge.call_arity = Some(arity);
    graph.add_edge(edge);

    if let Some(args) = node.child_by_field_name("arguments") {
        walk_body(&args, graph, ctx, method_id);
    }
    if let Some(obj) = node.child_by_field_name("object") {
        let raw = node_text(&obj, ctx.source).trim().to_owned();
        if !raw.is_empty() && !matches!(raw.as_str(), "this" | "super") {
            let base = raw.split('.').last().unwrap_or(&raw).to_owned();
            graph.add_edge_simple(EdgeKind::Reads, method_id, EdgeTarget::Unresolved(base), ts_span(&obj));
        }
    }
}

// ─── Instantiation ────────────────────────────────────────────────────────────

fn handle_instantiation(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    // type field holds the class being instantiated
    if let Some(type_node) = node.child_by_field_name("type") {
        let type_name = node_text(&type_node, ctx.source).to_owned();
        if !type_name.is_empty() {
            graph.add_edge_simple(
                EdgeKind::Instantiates, method_id, EdgeTarget::Unresolved(type_name), ts_span(&type_node),
            );
        }
    }
    if let Some(args) = node.child_by_field_name("arguments") {
        walk_body(&args, graph, ctx, method_id);
    }
}

// ─── Lambda ───────────────────────────────────────────────────────────────────

fn handle_lambda(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, method_id: NodeId) {
    let qname = qualify(&ctx.scope, "<lambda>");
    let mut closure = Node::new(0, NodeKind::Closure, "<lambda>", &qname, ctx.file, ts_span(node), Language::Java);
    closure.metadata.insert("enclosing_method".into(), method_id.to_string());
    let cid = graph.add_node(closure);

    graph.add_edge_simple(EdgeKind::Contains, method_id, EdgeTarget::Resolved(cid), ts_span(node));

    let child_ctx = Ctx { enclosing_method: Some(cid), ..ctx.child_scope("<lambda>") };
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, &child_ctx, cid);
    }
}
