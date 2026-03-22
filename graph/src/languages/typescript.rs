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

// ─── Visibility from accessibility modifier ───────────────────────────────────

fn visibility_from_accessibility(node: &TsNode, source: &[u8]) -> Visibility {
    let mut c = node.walk();
    node.children(&mut c)
        .find(|n| n.kind() == "accessibility_modifier")
        .map(|m| match node_text(&m, source) {
            "public"    => Visibility::Public,
            "protected" => Visibility::Protected,
            "private"   => Visibility::Private,
            _           => Visibility::Public,
        })
        .unwrap_or(Visibility::Public)
}

// ─── Type parameters ──────────────────────────────────────────────────────────

fn collect_type_params(node: &TsNode, source: &[u8]) -> Vec<String> {
    let Some(tp) = node.child_by_field_name("type_parameters") else { return vec![] };
    let mut c = tp.walk();
    tp.children(&mut c)
        .filter(|n| n.kind() == "type_parameter")
        .filter_map(|p| {
            let mut c2 = p.walk();
            p.children(&mut c2)
                .find(|n| n.kind() == "type_identifier")
                .map(|n| node_text(&n, source).to_owned())
        })
        .collect()
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

pub struct TypeScriptExtractor;

impl LanguageExtractor for TypeScriptExtractor {
    fn language(&self) -> Language { Language::TypeScript }

    fn extract(&self, source: &str, file: &str, graph: &mut DependencyGraph) {
        let src = source.as_bytes();
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("failed to load TypeScript grammar");

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
            Span::new(0, 0, 0, 0), Language::TypeScript,
        );
        file_node.visibility = Visibility::Public;
        file_node.hash       = Some(hash_source(source));
        let file_id = graph.add_node(file_node);

        let ctx = Ctx::new(src, file);
        walk_program(&tree.root_node(), graph, &ctx, file_id);
    }
}

// ─── Top-level walk ───────────────────────────────────────────────────────────

fn walk_program(root: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let mut pending_decorators: Vec<String> = vec![];
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement"                     => handle_import(&child, graph, ctx, file_id),
            "export_statement"                     => handle_export(&child, graph, ctx, file_id, &pending_decorators),
            "class_declaration"
            | "abstract_class_declaration"         => {
                handle_class(&child, graph, ctx, file_id, &pending_decorators);
                pending_decorators.clear();
            }
            "interface_declaration"                => {
                handle_interface(&child, graph, ctx, file_id);
                pending_decorators.clear();
            }
            "enum_declaration"                     => {
                handle_enum(&child, graph, ctx, file_id);
                pending_decorators.clear();
            }
            "function_declaration"
            | "generator_function_declaration"     => {
                handle_function(&child, graph, ctx, file_id, &pending_decorators, false); // not exported
                pending_decorators.clear();
            }
            "type_alias_declaration"               => {
                handle_type_alias(&child, graph, ctx, file_id);
                pending_decorators.clear();
            }
            "lexical_declaration" | "variable_declaration" => {
                handle_var_decl(&child, graph, ctx, file_id);
                pending_decorators.clear();
            }
            "decorator"                            => {
                pending_decorators.push(collect_decorator_text(&child, ctx.source));
            }
            _ => { pending_decorators.clear(); }
        }
    }
}

fn collect_decorator_text(node: &TsNode, source: &[u8]) -> String {
    let raw = node_text(node, source);
    raw.trim_start_matches('@').to_owned()
}

// ─── Export statement ─────────────────────────────────────────────────────────

fn handle_export(
    node:       &TsNode,
    graph:      &mut DependencyGraph,
    ctx:        &Ctx,
    file_id:    NodeId,
    decorators: &[String],
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration"
            | "abstract_class_declaration"     => handle_class(&child, graph, ctx, file_id, decorators),
            "interface_declaration"            => handle_interface(&child, graph, ctx, file_id),
            "enum_declaration"                 => handle_enum(&child, graph, ctx, file_id),
            "function_declaration"
            | "generator_function_declaration" => handle_function(&child, graph, ctx, file_id, decorators, true), // exported
            "type_alias_declaration"           => handle_type_alias(&child, graph, ctx, file_id),
            "lexical_declaration"
            | "variable_declaration"           => handle_var_decl(&child, graph, ctx, file_id),
            // `export { X, Y }` or `export { X } from '...'`
            "export_clause"                    => handle_export_clause(&child, graph, ctx, file_id, node),
            _ => {}
        }
    }
}

fn handle_export_clause(
    clause:  &TsNode,
    graph:   &mut DependencyGraph,
    ctx:     &Ctx,
    file_id: NodeId,
    parent:  &TsNode,
) {
    // Check if there's a `from '...'` source.
    let source_module = find_child(parent, "string")
        .map(|n| node_text(&n, ctx.source).trim_matches('"').trim_matches('\'').to_owned());

    let mut cursor = clause.walk();
    for item in clause.children(&mut cursor) {
        if item.kind() != "export_specifier" { continue; }
        let name = field_text(&item, "name", ctx.source).unwrap_or("").to_owned();
        if name.is_empty() { continue; }
        let alias = field_text(&item, "alias", ctx.source).map(str::to_owned);

        if let Some(ref src_mod) = source_module {
            let target = format!("{}.{}", src_mod, name);
            graph.add_edge_simple(EdgeKind::Reexports, file_id, EdgeTarget::External(target), ts_span(&item));
        } else {
            let target = alias.as_deref().unwrap_or(&name).to_owned();
            graph.add_edge_simple(EdgeKind::Reexports, file_id, EdgeTarget::Unresolved(target), ts_span(&item));
        }
    }
}

// ─── Import ───────────────────────────────────────────────────────────────────

fn handle_import(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let source_module = find_child(node, "string")
        .map(|n| node_text(&n, ctx.source).trim_matches('"').trim_matches('\'').to_owned())
        .unwrap_or_default();
    if source_module.is_empty() { return; }

    if let Some(clause) = find_child(node, "import_clause") {
        let mut cursor = clause.walk();
        for child in clause.children(&mut cursor) {
            match child.kind() {
                "identifier" => {
                    // default import: `import Foo from '...'`
                    let name = node_text(&child, ctx.source).to_owned();
                    emit_import(graph, ctx, file_id, &name, &source_module, ts_span(&child));
                }
                "named_imports" => {
                    // named: `import { X, Y as Z } from '...'`
                    let mut c = child.walk();
                    for spec in child.children(&mut c) {
                        if spec.kind() != "import_specifier" { continue; }
                        let name = field_text(&spec, "name", ctx.source).unwrap_or("").to_owned();
                        if name.is_empty() { continue; }
                        let alias = field_text(&spec, "alias", ctx.source).map(str::to_owned);
                        let display = alias.as_deref().unwrap_or(&name).to_owned();
                        let qpath = format!("{}.{}", source_module, name);
                        emit_import(graph, ctx, file_id, &display, &qpath, ts_span(&spec));
                    }
                }
                "namespace_import" => {
                    // `import * as ns from '...'`
                    let name = find_child(&child, "identifier")
                        .map(|n| node_text(&n, ctx.source).to_owned())
                        .unwrap_or_else(|| source_module.rsplit('/').next().unwrap_or(&source_module).to_owned());
                    let mut imp = Node::new(
                        0, NodeKind::Import, &name, &source_module,
                        ctx.file, ts_span(&child), Language::TypeScript,
                    );
                    imp.metadata.insert("namespace".into(), "true".into());
                    let imp_id = graph.add_node(imp);
                    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), ts_span(&child));
                    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(source_module.clone()), ts_span(&child));
                }
                _ => {}
            }
        }
    } else {
        // side-effect import: `import '...'`
        emit_import(
            graph, ctx, file_id,
            source_module.rsplit('/').next().unwrap_or(&source_module),
            &source_module,
            ts_span(node),
        );
    }
}

fn emit_import(
    graph:   &mut DependencyGraph,
    ctx:     &Ctx,
    file_id: NodeId,
    name:    &str,
    qpath:   &str,
    span:    Span,
) {
    let mut imp = Node::new(
        0, NodeKind::Import, name, qpath,
        ctx.file, span.clone(), Language::TypeScript,
    );
    let imp_id = graph.add_node(imp);
    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::Resolved(imp_id), span.clone());
    graph.add_edge_simple(EdgeKind::Imports, file_id, EdgeTarget::External(qpath.to_owned()), span);
}

// ─── Type alias ───────────────────────────────────────────────────────────────

fn handle_type_alias(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname      = qualify(&ctx.scope, &name);
    let type_params = collect_type_params(node, ctx.source);
    let value_text  = field_text(node, "value", ctx.source)
        .or_else(|| {
            // Older tree-sitter-typescript uses unnamed child after '='
            let mut c = node.walk();
            let children: Vec<_> = node.children(&mut c).collect();
            children.into_iter().rev()
                .find(|n| n.is_named() && n.kind() != "type_identifier" && n.kind() != "type_parameters")
                .map(|n| node_text(&n, ctx.source))
        })
        .map(str::to_owned);

    let mut ta = Node::new(0, NodeKind::TypeAlias, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    ta.visibility      = Visibility::Public;
    ta.generic_params  = type_params;
    ta.type_annotation = value_text.clone();
    let ta_id = graph.add_node(ta);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(ta_id), ts_span(node));
    if let Some(t) = value_text {
        graph.add_edge_simple(EdgeKind::References, ta_id, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

// ─── Enum ─────────────────────────────────────────────────────────────────────

fn handle_enum(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname = qualify(&ctx.scope, &name);

    let mut en = Node::new(0, NodeKind::Enum, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    en.visibility = Visibility::Public;
    let enum_id = graph.add_node(en);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(enum_id), ts_span(node));

    if let Some(body) = find_child(node, "enum_body") {
        let child_ctx = ctx.child_scope(&name);
        let mut cursor = body.walk();
        for member in body.children(&mut cursor) {
            if member.kind() != "enum_member" && member.kind() != "property_identifier" { continue; }
            let mname = if member.kind() == "enum_member" {
                field_text(&member, "name", ctx.source).unwrap_or("").to_owned()
            } else {
                node_text(&member, ctx.source).to_owned()
            };
            if mname.is_empty() { continue; }
            let mqname = qualify(&child_ctx.scope, &mname);
            let mut c = Node::new(0, NodeKind::Constant, &mname, &mqname, ctx.file, ts_span(&member), Language::TypeScript);
            c.visibility = Visibility::Public;
            let cid = graph.add_node(c);
            graph.add_edge_simple(EdgeKind::Contains, enum_id, EdgeTarget::Resolved(cid), ts_span(&member));
        }
    }
}

// ─── Interface ────────────────────────────────────────────────────────────────

fn handle_interface(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname       = qualify(&ctx.scope, &name);
    let type_params = collect_type_params(node, ctx.source);

    let mut iface = Node::new(0, NodeKind::Interface, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    iface.visibility    = Visibility::Public;
    iface.generic_params = type_params;
    let iface_id = graph.add_node(iface);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(iface_id), ts_span(node));

    // extends_type_clause → comma-separated type list
    if let Some(ext) = find_child(node, "extends_type_clause") {
        let mut c = ext.walk();
        for t in ext.children(&mut c) {
            if !t.is_named() { continue; }
            let tname = node_text(&t, ctx.source).to_owned();
            if !tname.is_empty() && tname != "extends" {
                graph.add_edge_simple(EdgeKind::Extends, iface_id, EdgeTarget::Unresolved(tname), ts_span(&t));
            }
        }
    }

    // Interface body: method signatures + property signatures
    let child_ctx = ctx.child_scope(&name);
    if let Some(body) = node.child_by_field_name("body") {
        walk_interface_body(&body, graph, &child_ctx, iface_id);
    }
}

fn walk_interface_body(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, iface_id: NodeId) {
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "method_signature" | "call_signature" => {
                let name = field_text(&child, "name", ctx.source).unwrap_or("").to_owned();
                if name.is_empty() { continue; }
                let qname = qualify(&ctx.scope, &name);
                let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(&child), Language::TypeScript);
                m.visibility  = Visibility::Public;
                m.is_abstract = true;
                m.generic_params = collect_type_params(&child, ctx.source);
                if let Some(ret) = find_child(&child, "type_annotation") {
                    m.type_annotation = Some(node_text(&ret, ctx.source)
                        .trim_start_matches(':')
                        .trim()
                        .to_owned());
                }
                let mid = graph.add_node(m);
                graph.add_edge_simple(EdgeKind::Contains, iface_id, EdgeTarget::Resolved(mid), ts_span(&child));
            }
            "property_signature" => {
                let name = field_text(&child, "name", ctx.source).unwrap_or("").to_owned();
                if name.is_empty() { continue; }
                let qname = qualify(&ctx.scope, &name);
                let type_ann = find_child(&child, "type_annotation")
                    .map(|n| node_text(&n, ctx.source).trim_start_matches(':').trim().to_owned());
                let mut f = Node::new(0, NodeKind::Field, &name, &qname, ctx.file, ts_span(&child), Language::TypeScript);
                f.visibility      = Visibility::Public;
                f.type_annotation = type_ann;
                let fid = graph.add_node(f);
                graph.add_edge_simple(EdgeKind::Contains, iface_id, EdgeTarget::Resolved(fid), ts_span(&child));
            }
            _ => {}
        }
    }
}

// ─── Class ────────────────────────────────────────────────────────────────────

fn handle_class(
    node:       &TsNode,
    graph:      &mut DependencyGraph,
    ctx:        &Ctx,
    file_id:    NodeId,
    decorators: &[String],
) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname       = qualify(&ctx.scope, &name);
    let is_abstract = node.kind() == "abstract_class_declaration";
    let type_params = collect_type_params(node, ctx.source);

    let mut cls = Node::new(0, NodeKind::Class, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    cls.visibility    = Visibility::Public;
    cls.is_abstract   = is_abstract;
    cls.generic_params = type_params;
    cls.attributes    = decorators.to_vec();
    if is_abstract {
        cls.metadata.insert("abstract".into(), "true".into());
    }
    let cls_id = graph.add_node(cls);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(cls_id), ts_span(node));

    // Decorates edges
    for dec in decorators {
        let dec_name = dec.trim_start_matches('@').to_owned();
        graph.add_edge_simple(EdgeKind::Decorates, cls_id, EdgeTarget::Unresolved(dec_name), ts_span(node));
    }

    // Heritage: extends and implements
    if let Some(heritage) = find_child(node, "class_heritage") {
        if let Some(ext) = find_child(&heritage, "extends_clause") {
            let mut c = ext.walk();
            for t in ext.children(&mut c) {
                if !t.is_named() { continue; }
                let tname = node_text(&t, ctx.source).to_owned();
                if !tname.is_empty() && tname != "extends" {
                    graph.add_edge_simple(EdgeKind::Extends, cls_id, EdgeTarget::Unresolved(tname), ts_span(&t));
                    break; // TypeScript only extends one class
                }
            }
        }
        if let Some(impl_clause) = find_child(&heritage, "implements_clause") {
            let mut c = impl_clause.walk();
            for t in impl_clause.children(&mut c) {
                if !t.is_named() { continue; }
                let tname = node_text(&t, ctx.source).to_owned();
                if !tname.is_empty() && tname != "implements" {
                    graph.add_edge_simple(EdgeKind::Implements, cls_id, EdgeTarget::Unresolved(tname), ts_span(&t));
                }
            }
        }
    }

    let child_ctx = ctx.child_scope(&name);
    if let Some(body) = find_child(node, "class_body") {
        walk_class_body(&body, graph, &child_ctx, cls_id);
    }
}

fn walk_class_body(body: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, cls_id: NodeId) {
    let mut pending_decorators: Vec<String> = vec![];
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "decorator" => {
                pending_decorators.push(collect_decorator_text(&child, ctx.source));
            }
            "method_definition" | "abstract_method_signature" => {
                handle_class_method(&child, graph, ctx, cls_id, &pending_decorators);
                pending_decorators.clear();
            }
            "public_field_definition" => {
                handle_class_field(&child, graph, ctx, cls_id);
                pending_decorators.clear();
            }
            _ => { pending_decorators.clear(); }
        }
    }
}

fn handle_class_method(
    node:       &TsNode,
    graph:      &mut DependencyGraph,
    ctx:        &Ctx,
    cls_id:     NodeId,
    decorators: &[String],
) {
    let name_node = node.child_by_field_name("name");
    let name = name_node
        .map(|n| node_text(&n, ctx.source).to_owned())
        .unwrap_or_default();
    if name.is_empty() || name == "constructor" && node.kind() != "method_definition" { return; }

    let qname         = qualify(&ctx.scope, &name);
    let is_constructor = name == "constructor";
    let is_abstract   = node.kind() == "abstract_method_signature"
        || node_text(node, ctx.source).contains("abstract");
    let is_async      = {
        let mut c = node.walk();
        node.children(&mut c).any(|n| n.kind() == "async")
    };
    let is_static     = {
        let mut c = node.walk();
        node.children(&mut c).any(|n| n.kind() == "static")
    };
    let visibility    = visibility_from_accessibility(node, ctx.source);
    let type_params   = collect_type_params(node, ctx.source);

    let mut m = Node::new(0, NodeKind::Method, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    m.visibility     = visibility;
    m.is_constructor = is_constructor;
    m.is_abstract    = is_abstract;
    m.is_async       = is_async;
    m.generic_params = type_params;
    m.attributes     = decorators.to_vec();
    if is_static { m.metadata.insert("static".into(), "true".into()); }

    if let Some(ret) = find_child(node, "type_annotation") {
        m.type_annotation = Some(node_text(&ret, ctx.source)
            .trim_start_matches(':')
            .trim()
            .to_owned());
    }

    let mid = graph.add_node(m);
    graph.add_edge_simple(EdgeKind::Contains, cls_id, EdgeTarget::Resolved(mid), ts_span(node));

    // Decorates edges
    for dec in decorators {
        let dec_name = dec.trim_start_matches('@').to_owned();
        graph.add_edge_simple(EdgeKind::Decorates, mid, EdgeTarget::Unresolved(dec_name), ts_span(node));
    }

    // Return type edge
    if !is_constructor {
        if let Some(ret) = find_child(node, "type_annotation") {
            let rt = node_text(&ret, ctx.source).trim_start_matches(':').trim().to_owned();
            if !rt.is_empty() && rt != "void" {
                graph.add_edge_simple(EdgeKind::Returns, mid, EdgeTarget::Unresolved(rt), ts_span(node));
            }
        }
    }

    // Parameters
    let child_ctx = ctx.child_scope(&name);
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_formal_params(&params, graph, &child_ctx, mid);
    }

    // Body
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, &child_ctx, mid);
    }
}

fn handle_class_field(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, cls_id: NodeId) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname      = qualify(&ctx.scope, &name);
    let visibility = visibility_from_accessibility(node, ctx.source);
    let is_static  = {
        let mut c = node.walk();
        node.children(&mut c).any(|n| n.kind() == "static")
    };
    let type_ann = find_child(node, "type_annotation")
        .map(|n| node_text(&n, ctx.source).trim_start_matches(':').trim().to_owned());

    let kind = if is_static { NodeKind::StaticField } else { NodeKind::Field };
    let mut f = Node::new(0, kind, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    f.visibility      = visibility;
    f.type_annotation = type_ann.clone();
    if is_static { f.metadata.insert("static".into(), "true".into()); }
    let fid = graph.add_node(f);
    graph.add_edge_simple(EdgeKind::Contains, cls_id, EdgeTarget::Resolved(fid), ts_span(node));
    if let Some(t) = type_ann {
        graph.add_edge_simple(EdgeKind::HasType, fid, EdgeTarget::Unresolved(t), ts_span(node));
    }
}

// ─── Top-level function ───────────────────────────────────────────────────────

fn handle_function(
    node:        &TsNode,
    graph:       &mut DependencyGraph,
    ctx:         &Ctx,
    file_id:     NodeId,
    decorators:  &[String],
    is_exported: bool,
) {
    let name = field_text(node, "name", ctx.source).unwrap_or("").to_owned();
    if name.is_empty() { return; }
    let qname       = qualify(&ctx.scope, &name);
    let type_params = collect_type_params(node, ctx.source);
    let is_async    = {
        let mut c = node.walk();
        node.children(&mut c).any(|n| n.kind() == "async")
    };

    let mut f = Node::new(0, NodeKind::Function, &name, &qname, ctx.file, ts_span(node), Language::TypeScript);
    f.visibility    = if is_exported { Visibility::Public } else { Visibility::Private };
    f.is_async      = is_async;
    f.generic_params = type_params;
    f.attributes    = decorators.to_vec();
    if let Some(ret) = find_child(node, "type_annotation") {
        f.type_annotation = Some(node_text(&ret, ctx.source).trim_start_matches(':').trim().to_owned());
    }
    let fid = graph.add_node(f);
    graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(fid), ts_span(node));

    if let Some(ret) = find_child(node, "type_annotation") {
        let rt = node_text(&ret, ctx.source).trim_start_matches(':').trim().to_owned();
        if !rt.is_empty() && rt != "void" {
            graph.add_edge_simple(EdgeKind::Returns, fid, EdgeTarget::Unresolved(rt), ts_span(node));
        }
    }

    let child_ctx = ctx.child_scope(&name);
    if let Some(params) = node.child_by_field_name("parameters") {
        handle_formal_params(&params, graph, &child_ctx, fid);
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk_body(&body, graph, &child_ctx, fid);
    }
}

// ─── Top-level variable/const declaration ─────────────────────────────────────

fn handle_var_decl(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, file_id: NodeId) {
    let is_const = {
        let mut c = node.walk();
        node.children(&mut c).any(|n| n.kind() == "const")
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "variable_declarator" { continue; }
        let name = field_text(&child, "name", ctx.source).unwrap_or("").to_owned();
        if name.is_empty() { continue; }
        let qname    = qualify(&ctx.scope, &name);
        let type_ann = find_child(&child, "type_annotation")
            .map(|n| node_text(&n, ctx.source).trim_start_matches(':').trim().to_owned());

        let kind = if is_const { NodeKind::Constant } else { NodeKind::GlobalVariable };
        let mut v = Node::new(0, kind, &name, &qname, ctx.file, ts_span(&child), Language::TypeScript);
        v.visibility      = Visibility::Public;
        v.type_annotation = type_ann;
        let vid = graph.add_node(v);
        graph.add_edge_simple(EdgeKind::Contains, file_id, EdgeTarget::Resolved(vid), ts_span(&child));
    }
}

// ─── Parameters ───────────────────────────────────────────────────────────────

fn handle_formal_params(params: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = params.walk();
    for param in params.children(&mut cursor) {
        match param.kind() {
            "required_parameter" | "optional_parameter" => {
                let name = param.child_by_field_name("pattern")
                    .or_else(|| find_child(&param, "identifier"))
                    .map(|n| node_text(&n, ctx.source).to_owned())
                    .unwrap_or_default();
                if name.is_empty() || name == "this" { continue; }

                let type_ann = find_child(&param, "type_annotation")
                    .map(|n| node_text(&n, ctx.source).trim_start_matches(':').trim().to_owned());

                let qname = qualify(&ctx.scope, &name);
                let mut p = Node::new(0, NodeKind::Parameter, &name, &qname, ctx.file, ts_span(&param), Language::TypeScript);
                p.type_annotation = type_ann.clone();
                let pid = graph.add_node(p);
                graph.add_edge_simple(EdgeKind::HasParameter, fn_id, EdgeTarget::Resolved(pid), ts_span(&param));
                if let Some(t) = type_ann {
                    graph.add_edge_simple(EdgeKind::HasType, pid, EdgeTarget::Unresolved(t), ts_span(&param));
                }
            }
            _ => {}
        }
    }
}

// ─── Body / expressions ───────────────────────────────────────────────────────

fn walk_body(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        dispatch_expr(&child, graph, ctx, fn_id);
    }
}

fn dispatch_expr(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    match node.kind() {
        "call_expression"          => handle_call(node, graph, ctx, fn_id),
        "new_expression"           => handle_new(node, graph, ctx, fn_id),
        "await_expression"         => {
            if let Some(inner) = node.named_child(0) {
                if inner.kind() == "call_expression" {
                    handle_call(&inner, graph, ctx, fn_id);
                    let callee = inner.child_by_field_name("function")
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

fn handle_new(node: &TsNode, graph: &mut DependencyGraph, ctx: &Ctx, fn_id: NodeId) {
    if let Some(ctor) = node.child_by_field_name("constructor") {
        let type_name = node_text(&ctor, ctx.source).to_owned();
        if !type_name.is_empty() {
            graph.add_edge_simple(EdgeKind::Instantiates, fn_id, EdgeTarget::Unresolved(type_name), ts_span(&ctor));
        }
    }
    if let Some(args) = node.child_by_field_name("arguments") {
        walk_body(&args, graph, ctx, fn_id);
    }
}
