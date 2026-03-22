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

/// Go identifiers starting with an uppercase letter are exported (public).
fn go_visibility(name: &str) -> Visibility {
    if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        Visibility::Public
    } else {
        Visibility::Internal
    }
}

// ─── Context ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Ctx<'a> {
    source:  &'a [u8],
    file:    &'a str,
    scope:   String,
}

impl<'a> Ctx<'a> {
    fn new(source: &'a [u8], file: &'a str) -> Self {
        Self { source, file, scope: String::new() }
    }
    fn with_scope(&self, scope: &str) -> Self {
        Self { scope: scope.to_owned(), ..self.clone() }
    }
    fn child_scope(&self, name: &str) -> Self {
        Self { scope: qualify(&self.scope, name), ..self.clone() }
    }
}

// ─── Extractor ────────────────────────────────────────────────────────────────

pub struct GoExtractor;

impl LanguageExtractor for GoExtractor {
    fn language(&self) -> Language { Language::Go }

    fn extract(&self, source: &str, file: &str, graph: &mut DependencyGraph) {
        let src = source.as_bytes();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("failed to load Go grammar");

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
            Span::new(0, 0, 0, 0), Language::Go,
        );
        file_node.visibility = Visibility::Public;
        file_node.hash       = Some(hash_source(source));
        let file_id = graph.add_node(file_node);

        // First pass: find package name to use as top-level scope.
        let pkg_name = {
            let mut cursor = tree.root_node().walk();
            let pkg_clause = tree.root_node().children(&mut cursor)
                .find(|n| n.kind() == "package_clause");
            if let Some(clause) = pkg_clause {
                find_child(&clause, "package_identifier")
                    .map(|n| node_text(&n, src).to_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        };

        let ctx = Ctx::new(src, file).with_scope(&pkg_name);
        walk_source_file(&tree.root_node(), graph, &ctx, file_id);
    }
}

// ─── Top-level walk ───────────────────────────────────────────────────────────

fn walk_source_file(root: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "package_clause"       => handle_package(&child, graph, ctx, file_id),
            "import_declaration"   => handle_import(&child, graph, ctx, file_id),
            "type_declaration"     => handle_type_decl(&child, graph, ctx, file_id),
            "function_declaration" => handle_function(&child, graph, ctx, file_id),
            "method_declaration"   => handle_method_decl(&child, graph, ctx, file_id),
            "const_declaration"    => handle_const_decl(&child, graph, ctx, file_id),
            "var_declaration"      => handle_var_decl(&child, graph, ctx, file_id),
            _ => {}
        }
    }
}

// ─── Package ──────────────────────────────────────────────────────────────────

fn handle_package(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let Some(id_node) = find_child(node, "package_identifier") else { return };
    let name = node_text(&id_node, ctx.source).to_owned();
    if name.is_empty() { return; }
    let mut pkg = Node::new(
        0, NodeKind::Package, &name, &name,
        ctx.file, ts_span(node), Language::Go,
    );
    pkg.visibility = Visibility::Public;
    let pid = graph.add_node(pkg);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(pid), ts_span(node));
}

// ─── Import ───────────────────────────────────────────────────────────────────

fn handle_import(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    // Gather specs avoiding lifetime issues with temporary TsNode bindings.
    let direct_specs = children_of_kind(node, "import_spec");
    let has_list = find_child(node, "import_spec_list").is_some();

    let spec_iter: Box<dyn Iterator<Item = TsNode>> = if has_list {
        // Re-find the list to avoid borrowing a temporary.
        if let Some(list_node) = find_child(node, "import_spec_list") {
            // Collect into owned indices so we don't borrow `list_node`.
            let count = list_node.child_count();
            let mut specs = Vec::with_capacity(count);
            let mut c = list_node.walk();
            for child in list_node.children(&mut c) {
                if child.kind() == "import_spec" {
                    specs.push(child);
                }
            }
            Box::new(specs.into_iter())
        } else {
            Box::new(direct_specs.into_iter())
        }
    } else {
        Box::new(direct_specs.into_iter())
    };

    for spec in spec_iter {
        // path field is an interpreted_string_literal
        let path = field_text(&spec, "path", ctx.source)
            .map(|s| s.trim_matches('"').trim_matches('`').to_owned())
            .unwrap_or_default();
        if path.is_empty() { continue; }

        // optional name/alias field: package_identifier | "." | "_"
        let alias = field_text(&spec, "name", ctx.source);

        let simple = alias.unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path));
        if simple == "_" { continue; } // blank import — skip

        let mut imp = Node::new(
            0, NodeKind::Import, simple, &path,
            ctx.file, ts_span(&spec), Language::Go,
        );
        if let Some(a) = alias {
            imp.metadata.insert("alias".into(), a.to_owned());
        }
        let imp_id = graph.add_node(imp);
        graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(&spec));
        graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(path), ts_span(&spec));
    }
}

// ─── Type declarations ────────────────────────────────────────────────────────

fn handle_type_decl(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_spec"  => handle_type_spec(&child, graph, ctx, file_id),
            "type_alias" => handle_type_alias(&child, graph, ctx, file_id),
            _ => {}
        }
    }
}

fn handle_type_spec(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname      = qualify(&ctx.scope, &name);
    let visibility = go_visibility(&name);
    let type_params = collect_type_params(node, ctx.source);

    // Determine body kind: struct_type → Class, interface_type → Interface, else TypeAlias.
    let mut cursor = node.walk();
    let body_kind = node.children(&mut cursor)
        .find(|n| matches!(n.kind(), "struct_type" | "interface_type"))
        .map(|n| n.kind().to_owned());

    match body_kind.as_deref() {
        Some("struct_type") => {
            let mut cls = Node::new(0, NodeKind::Class, &name, &qname, ctx.file, ts_span(node), Language::Go);
            cls.visibility    = visibility;
            cls.generic_params = type_params;
            let cls_id = graph.add_node(cls);
            graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(cls_id), ts_span(node));
            if let Some(struct_type) = find_child(node, "struct_type") {
                if let Some(fdl) = find_child(&struct_type, "field_declaration_list") {
                    handle_struct_fields(&fdl, graph, &ctx.child_scope(&name), cls_id);
                }
            }
        }
        Some("interface_type") => {
            let mut iface = Node::new(0, NodeKind::Interface, &name, &qname, ctx.file, ts_span(node), Language::Go);
            iface.visibility    = visibility;
            iface.generic_params = type_params;
            let iface_id = graph.add_node(iface);
            graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(iface_id), ts_span(node));
            if let Some(iface_type) = find_child(node, "interface_type") {
                handle_interface_body(&iface_type, graph, &ctx.child_scope(&name), iface_id);
            }
        }
        _ => {
            // Named type (e.g. `type Miles float64`).
            let underlying: Option<String> = {
                let mut c = node.walk();
                node.children(&mut c)
                    .find(|n| n.is_named() && !matches!(n.kind(), "type_identifier" | "type_parameter_list"))
                    .map(|n| node_text(&n, ctx.source).to_owned())
            };
            let mut ta = Node::new(0, NodeKind::TypeAlias, &name, &qname, ctx.file, ts_span(node), Language::Go);
            ta.visibility      = visibility;
            ta.type_annotation = underlying.clone();
            let ta_id = graph.add_node(ta);
            graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(ta_id), ts_span(node));
            if let Some(t) = underlying {
                graph.add_edge_simple(EdgeKind::References, ta_id, EdgeTarget::Unresolved(t), ts_span(node));
            }
        }
    }
}

fn handle_type_alias(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);
    let type_text: Option<String> = {
        let mut c = node.walk();
        node.children(&mut c)
            .find(|n| n.is_named() && n.kind() != "type_identifier")
            .map(|n| node_text(&n, ctx.source).to_owned())
    };
    let mut ta = Node::new(0, NodeKind::TypeAlias, &name, &qname, ctx.file, ts_span(node), Language::Go);
    ta.visibility      = go_visibility(&name);
    ta.type_annotation = type_text.clone();
    ta.metadata.insert("true_alias".into(), "true".into());
    let ta_id = graph.add_node(ta);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(ta_id), ts_span(node));
    if let Some(t) = type_text {
        graph.add_edge_simple(EdgeKind::References, ta_id, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

fn collect_type_params(node: &TsNode, source: &[u8]) -> Vec<String> {
    let Some(tp) = node.child_by_field_name("type_parameters") else { return vec![] };
    let mut c = tp.walk();
    tp.children(&mut c)
        .filter(|n| n.kind() == "type_parameter_declaration")
        .filter_map(|tpd| tpd.child_by_field_name("name").map(|n| node_text(&n, source).to_owned()))
        .collect()
}

// ─── Struct fields ────────────────────────────────────────────────────────────

fn handle_struct_fields(fdl: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, struct_id: NodeId) {
    let mut cursor = fdl.walk();
    for fd in fdl.children(&mut cursor) {
        if fd.kind() != "field_declaration" { continue; }

        let type_text = field_text(&fd, "type", ctx.source).map(str::to_owned);
        let name_node = fd.child_by_field_name("name");

        if let Some(name_list) = name_node {
            // Named field(s): `Name string` or `X, Y float64`.
            let names: Vec<String> = if name_list.kind() == "field_identifier_list" {
                let mut c = name_list.walk();
                name_list.children(&mut c)
                    .filter(|n| n.kind() == "field_identifier")
                    .map(|n| node_text(&n, ctx.source).to_owned())
                    .collect()
            } else {
                vec![node_text(&name_list, ctx.source).to_owned()]
            };

            for name in &names {
                if name.is_empty() { continue; }
                let qname = qualify(&ctx.scope, name);
                let mut f = Node::new(0, NodeKind::Field, name, &qname, ctx.file, ts_span(&fd), Language::Go);
                f.visibility      = go_visibility(name);
                f.type_annotation = type_text.clone();
                let fid = graph.add_node(f);
                graph.add_edge_simple(EdgeKind::Contains, struct_id, EdgeTarget::Resolved(fid), ts_span(&fd));
                if let Some(ref t) = type_text {
                    graph.add_edge_simple(EdgeKind::HasType, fid, EdgeTarget::Unresolved(t.clone()), ts_span(&fd));
                }
            }
        } else if let Some(ref t) = type_text {
            // Embedded type (anonymous field): add Extends edge.
            let embedded = t.trim_start_matches('*').to_owned();
            if !embedded.is_empty() {
                graph.add_edge_simple(
                    EdgeKind::Extends, struct_id, EdgeTarget::Unresolved(embedded), ts_span(&fd),
                );
            }
        }
    }
}

// ─── Interface body ───────────────────────────────────────────────────────────

fn handle_interface_body(iface_type: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, iface_id: NodeId) {
    let mut cursor = iface_type.walk();
    for child in iface_type.children(&mut cursor) {
        match child.kind() {
            // tree-sitter-go 0.23: method_elem for method signatures
            "method_elem" => {
                let name = field_text(&child, "name", ctx.source).unwrap_or("").to_owned();
                if name.is_empty() { continue; }
                let qname = qualify(&ctx.scope, &name);
                let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(&child), Language::Go);
                m.visibility = Visibility::Public;
                m.is_abstract = true;
                let mid = graph.add_node(m);
                graph.add_edge_simple(EdgeKind::Contains, iface_id, EdgeTarget::Resolved(mid), ts_span(&child));
                // Parameters
                if let Some(params) = child.child_by_field_name("parameters") {
                    handle_params(&params, graph, ctx, mid);
                }
                // Return type
                if let Some(result) = child.child_by_field_name("result") {
                    let result_text = node_text(&result, ctx.source).to_owned();
                    if !result_text.is_empty() {
                        graph.add_edge_simple(EdgeKind::Returns, mid, EdgeTarget::Unresolved(result_text), ts_span(&result));
                    }
                }
            }
            // tree-sitter-go 0.23: type_elem for embedded interface references
            "type_elem" => {
                let mut c = child.walk();
                for t in child.children(&mut c) {
                    if !t.is_named() { continue; }
                    let type_name = node_text(&t, ctx.source)
                        .trim_start_matches('~')
                        .to_owned();
                    if !type_name.is_empty() {
                        graph.add_edge_simple(EdgeKind::Extends, iface_id, EdgeTarget::Unresolved(type_name), ts_span(&t));
                    }
                }
            }
            // Legacy: method_spec (older grammar versions)
            "method_spec" => {
                let name = field_text(&child, "name", ctx.source).unwrap_or("").to_owned();
                if name.is_empty() { continue; }
                let qname = qualify(&ctx.scope, &name);
                let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(&child), Language::Go);
                m.visibility = Visibility::Public;
                m.is_abstract = true;
                let mid = graph.add_node(m);
                graph.add_edge_simple(EdgeKind::Contains, iface_id, EdgeTarget::Resolved(mid), ts_span(&child));
                if let Some(params) = child.child_by_field_name("parameters") {
                    handle_params(&params, graph, ctx, mid);
                }
            }
            // Embedded interface reference (older grammar)
            "interface_type_name" | "type_identifier" => {
                let type_name = node_text(&child, ctx.source).to_owned();
                if !type_name.is_empty() {
                    graph.add_edge_simple(EdgeKind::Extends, iface_id, EdgeTarget::Unresolved(type_name), ts_span(&child));
                }
            }
            _ => {}
        }
    }
}

// ─── Function declaration ─────────────────────────────────────────────────────

fn handle_function(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);

    let mut f = Node::new(0, NodeKind::Function, &name, &qname, ctx.file, ts_span(node), Language::Go);
    f.visibility = go_visibility(&name);
    if let Some(result) = node.child_by_field_name("result") {
        f.type_annotation = Some(node_text(&result, ctx.source).to_owned());
    }
    let fid = graph.add_node(f);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(fid), ts_span(node));

    if let Some(result) = node.child_by_field_name("result") {
        let rt = node_text(&result, ctx.source).to_owned();
        if !rt.is_empty() {
            graph.add_edge_simple(EdgeKind::Returns, fid, EdgeTarget::Unresolved(rt), ts_span(&result));
        }
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_params(&params, graph, ctx, fid);
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, ctx, fid);
    }
}

// ─── Method declaration ───────────────────────────────────────────────────────

fn handle_method_decl(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }

    // Extract receiver type for qualification.
    let recv_type = extract_receiver_type(node, ctx.source);

    let qname = if recv_type.is_empty() {
        qualify(&ctx.scope, &name)
    } else {
        format!("{}.{}", qualify(&ctx.scope, &recv_type), name)
    };

    let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(node), Language::Go);
    m.visibility = go_visibility(&name);
    if !recv_type.is_empty() {
        m.metadata.insert("receiver".into(), recv_type.clone());
    }
    let mid = graph.add_node(m);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(mid), ts_span(node));

    if let Some(result) = node.child_by_field_name("result") {
        let rt = node_text(&result, ctx.source).to_owned();
        if !rt.is_empty() {
            graph.add_edge_simple(EdgeKind::Returns, mid, EdgeTarget::Unresolved(rt), ts_span(&result));
        }
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_params(&params, graph, ctx, mid);
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, ctx, mid);
    }
}

fn extract_receiver_type(method_node: &TsNode, source: &[u8]) -> String {
    method_node
        .child_by_field_name("receiver")
        .and_then(|recv| {
            let mut c = recv.walk();
            recv.children(&mut c).find(|n| n.kind() == "parameter_declaration")
        })
        .and_then(|pd| pd.child_by_field_name("type"))
        .map(|t| node_text(&t, source).trim_start_matches('*').to_owned())
        .unwrap_or_default()
}

// ─── Parameters ───────────────────────────────────────────────────────────────

fn handle_params(params: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = params.walk();
    for param in params.children(&mut cursor) {
        match param.kind() {
            "parameter_declaration" | "variadic_parameter_declaration" => {
                let type_text = param.child_by_field_name("type").map(|n| node_text(&n, ctx.source).to_owned());

                // Names come from the `name` field which is an identifier_list.
                let names: Vec<String> = param.child_by_field_name("name")
                    .map(|nl| {
                        if nl.kind() == "identifier_list" {
                            let mut c = nl.walk();
                            nl.children(&mut c)
                                .filter(|n| n.kind() == "identifier")
                                .map(|n| node_text(&n, ctx.source).to_owned())
                                .collect()
                        } else {
                            vec![node_text(&nl, ctx.source).to_owned()]
                        }
                    })
                    .unwrap_or_default();

                for pname in &names {
                    if pname.is_empty() || pname == "_" { continue; }
                    let qname = qualify(&ctx.scope, pname);
                    let mut p = Node::new(
                        0, NodeKind::Parameter, pname, &qname,
                        ctx.file, ts_span(&param), Language::Go,
                    );
                    p.type_annotation = type_text.clone();
                    let pid = graph.add_node(p);
                    graph.add_edge_simple(EdgeKind::HasParameter, fn_id, EdgeTarget::Resolved(pid), ts_span(&param));
                    if let Some(ref t) = type_text {
                        graph.add_edge_simple(EdgeKind::HasType, pid, EdgeTarget::Unresolved(t.clone()), ts_span(&param));
                    }
                }
            }
            _ => {}
        }
    }
}

// ─── Const / var declarations ─────────────────────────────────────────────────

fn handle_const_decl(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut cursor = node.walk();
    for spec in node.children(&mut cursor) {
        if spec.kind() != "const_spec" { continue; }
        let type_text = spec.child_by_field_name("type").map(|n| node_text(&n, ctx.source).to_owned());
        for name in spec_names(&spec, ctx.source) {
            let qname = qualify(&ctx.scope, &name);
            let mut c = Node::new(0, NodeKind::Constant, &name, &qname, ctx.file, ts_span(&spec), Language::Go);
            c.visibility      = go_visibility(&name);
            c.type_annotation = type_text.clone();
            let cid = graph.add_node(c);
            graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(cid), ts_span(&spec));
        }
    }
}

fn handle_var_decl(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    // Collect all var_spec nodes: directly or inside a var_spec_list wrapper.
    let mut specs: Vec<TsNode> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "var_spec" => specs.push(child),
            "var_spec_list" => {
                let mut c2 = child.walk();
                for s in child.children(&mut c2) {
                    if s.kind() == "var_spec" { specs.push(s); }
                }
            }
            _ => {}
        }
    }
    for spec in specs {
        let type_text = spec.child_by_field_name("type").map(|n| node_text(&n, ctx.source).to_owned());
        for name in spec_names(&spec, ctx.source) {
            let qname = qualify(&ctx.scope, &name);
            let mut v = Node::new(0, NodeKind::GlobalVariable, &name, &qname, ctx.file, ts_span(&spec), Language::Go);
            v.visibility      = go_visibility(&name);
            v.type_annotation = type_text.clone();
            let vid = graph.add_node(v);
            graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(vid), ts_span(&spec));
        }
    }
}

/// Extract identifier names from a const_spec or var_spec node.
fn spec_names(spec: &TsNode, source: &[u8]) -> Vec<String> {
    spec.child_by_field_name("name")
        .map(|nl| {
            if nl.kind() == "identifier_list" {
                let mut c = nl.walk();
                nl.children(&mut c)
                    .filter(|n| n.kind() == "identifier")
                    .map(|n| node_text(&n, source).to_owned())
                    .collect()
            } else {
                vec![node_text(&nl, source).to_owned()]
            }
        })
        .unwrap_or_default()
}

// ─── Body / expressions ───────────────────────────────────────────────────────

fn walk_body(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_stmt(&child, graph, ctx, fn_id);
    }
}

fn dispatch_stmt(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    match node.kind() {
        "call_expression"       => handle_call(node, graph, ctx, fn_id),
        "composite_literal"     => handle_composite_literal(node, graph, ctx, fn_id),
        "short_var_declaration" => {
            if let Some(right) = node.child_by_field_name("right") {
                walk_body(&right, graph, ctx, fn_id);
            }
        }
        "assignment_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                let raw = node_text(&left, ctx.source);
                let base = raw.split('.').last().unwrap_or(raw).trim().to_owned();
                if !base.is_empty() {
                    graph.add_edge_simple(EdgeKind::Writes, fn_id, EdgeTarget::Unresolved(base), ts_span(&left));
                }
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk_body(&right, graph, ctx, fn_id);
            }
        }
        "expression_statement" => {
            let mut c = node.walk();
            for child in node.children(&mut c) {
                dispatch_stmt(&child, graph, ctx, fn_id);
            }
        }
        _ => walk_body(node, graph, ctx, fn_id),
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
        walk_body(&args, graph, ctx, fn_id);
    }
}

fn handle_composite_literal(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    if let Some(type_node) = node.child_by_field_name("type") {
        let type_name = node_text(&type_node, ctx.source)
            .trim_start_matches('&')
            .trim_start_matches('*')
            .to_owned();
        if !type_name.is_empty() {
            graph.add_edge_simple(EdgeKind::Instantiates, fn_id, EdgeTarget::Unresolved(type_name), ts_span(&type_node));
        }
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, ctx, fn_id);
    }
}
