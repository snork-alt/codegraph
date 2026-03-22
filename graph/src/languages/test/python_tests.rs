use std::collections::HashMap;

use crate::graph::{DependencyGraph, EdgeKind, EdgeTarget, Language, NodeId, NodeKind, Visibility};
use crate::languages::python::PythonExtractor;
use crate::parser::LanguageExtractor;

// ─── Fixture ──────────────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/shop.py");
const FILE: &str    = "shop/shop.py";

fn extract() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    PythonExtractor.extract(FIXTURE, FILE, &mut g);
    g.resolve();
    g
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn nodes_of_kind(g: &DependencyGraph, kind: &NodeKind) -> HashMap<String, NodeId> {
    g.nodes
        .values()
        .filter(|n| std::mem::discriminant(&n.kind) == std::mem::discriminant(kind))
        .map(|n| (n.name.clone(), n.id))
        .collect()
}

fn edges_from(g: &DependencyGraph, id: NodeId) -> Vec<&crate::graph::Edge> {
    g.edges_from
        .get(&id)
        .map(|ids| ids.iter().map(|&eid| &g.edges[eid as usize]).collect())
        .unwrap_or_default()
}

fn has_edge_to(g: &DependencyGraph, from: NodeId, kind: &EdgeKind, target: &str) -> bool {
    edges_from(g, from).into_iter().any(|e| {
        std::mem::discriminant(&e.kind) == std::mem::discriminant(kind)
            && match &e.to {
                EdgeTarget::Resolved(id) => g
                    .nodes.get(id)
                    .map(|n| n.name == target || n.qualified_name == target)
                    .unwrap_or(false),
                EdgeTarget::Unresolved(s) | EdgeTarget::External(s) =>
                    s == target || s.ends_with(&format!(".{}", target)),
            }
    })
}

fn has_edge_containing(g: &DependencyGraph, from: NodeId, kind: &EdgeKind, substr: &str) -> bool {
    edges_from(g, from).into_iter().any(|e| {
        std::mem::discriminant(&e.kind) == std::mem::discriminant(kind)
            && match &e.to {
                EdgeTarget::Resolved(id) => g.nodes.get(id)
                    .map(|n| n.name.contains(substr) || n.qualified_name.contains(substr))
                    .unwrap_or(false),
                EdgeTarget::Unresolved(s) | EdgeTarget::External(s) => s.contains(substr),
            }
    })
}

// ─── File node ────────────────────────────────────────────────────────────────

#[test]
fn file_node_exists() {
    let g = extract();
    let files = nodes_of_kind(&g, &NodeKind::File);
    assert!(files.contains_key("shop.py"));
}

#[test]
fn file_language_is_python() {
    let g = extract();
    let file = g.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
    assert_eq!(file.language, Language::Python);
}

#[test]
fn file_has_hash() {
    let g = extract();
    let file = g.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
    assert!(file.hash.is_some());
}

// ─── Imports ──────────────────────────────────────────────────────────────────

#[test]
fn imports_asyncio_and_logging() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("asyncio"), "expected 'asyncio' import");
    assert!(imports.contains_key("logging"), "expected 'logging' import");
}

#[test]
fn from_import_dataclass() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("dataclass"), "expected 'dataclass' import");
}

#[test]
fn from_import_optional() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("Optional"), "expected 'Optional' import");
}

#[test]
fn from_import_protocol() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("Protocol"), "expected 'Protocol' import");
}

// ─── Module-level constants ───────────────────────────────────────────────────

#[test]
fn constant_status_pending() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("STATUS_PENDING"), "expected constant 'STATUS_PENDING'");
}

#[test]
fn constant_max_items() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("MAX_ITEMS"), "expected constant 'MAX_ITEMS'");
}

#[test]
fn constants_are_public() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    if let Some(&id) = consts.get("STATUS_PENDING") {
        assert_eq!(g.nodes[&id].visibility, Visibility::Public);
    }
}

// ─── Module-level variable ────────────────────────────────────────────────────

#[test]
fn global_variable_registry() {
    let g = extract();
    let vars = nodes_of_kind(&g, &NodeKind::GlobalVariable);
    assert!(vars.contains_key("_registry"), "expected GlobalVariable '_registry'");
}

// ─── Classes ──────────────────────────────────────────────────────────────────

#[test]
fn class_item_not_found_error_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("ItemNotFoundError"));
}

#[test]
fn class_repository_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Repository"));
}

#[test]
fn class_item_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Item"));
}

#[test]
fn class_store_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Store"));
}

#[test]
fn item_not_found_error_extends_exception() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["ItemNotFoundError"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Extends, "Exception"),
        "ItemNotFoundError should extend Exception"
    );
}

#[test]
fn repository_extends_protocol() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Repository"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Extends, "Protocol"),
        "Repository should extend Protocol"
    );
}

// ─── Decorators ───────────────────────────────────────────────────────────────

#[test]
fn item_class_has_dataclass_decorator() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Item"];
    assert!(
        g.nodes[&id].attributes.iter().any(|a| a.contains("dataclass")),
        "Item should have @dataclass decorator"
    );
}

#[test]
fn item_class_has_decorates_edge() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Item"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Decorates, "dataclass"),
        "Item should have Decorates edge to dataclass"
    );
}

// ─── Instance fields (from __init__) ─────────────────────────────────────────

#[test]
fn store_has_repo_field() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("repo"), "expected Field 'repo'");
}

#[test]
fn store_has_name_field() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("name"), "expected Field 'name'");
}

#[test]
fn store_has_cache_field() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("_cache"), "expected private Field '_cache'");
}

#[test]
fn private_fields_have_private_visibility() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    if let Some(&id) = fields.get("_cache") {
        assert_eq!(g.nodes[&id].visibility, Visibility::Private);
    }
}

#[test]
fn public_fields_have_public_visibility() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    if let Some(&id) = fields.get("repo") {
        assert_eq!(g.nodes[&id].visibility, Visibility::Public);
    }
}

// ─── Methods ──────────────────────────────────────────────────────────────────

#[test]
fn store_has_add_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("add"));
}

#[test]
fn store_has_get_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("get"));
}

#[test]
fn store_has_async_refresh_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("refresh"), "expected Method 'refresh'");
    let id = methods["refresh"];
    assert!(g.nodes[&id].is_async, "refresh must be async");
}

#[test]
fn store_has_private_validate_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("_validate"));
    let id = methods["_validate"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Private);
}

#[test]
fn init_is_constructor() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("__init__"), "expected '__init__' method");
    let id = methods["__init__"];
    assert!(g.nodes[&id].is_constructor, "__init__ must be marked as constructor");
}

#[test]
fn item_create_is_classmethod() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("create") {
        assert_eq!(
            g.nodes[&id].metadata.get("classmethod").map(String::as_str),
            Some("true")
        );
    }
}

#[test]
fn item_validate_price_is_staticmethod() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("validate_price") {
        assert_eq!(
            g.nodes[&id].metadata.get("static").map(String::as_str),
            Some("true")
        );
    }
}

#[test]
fn is_available_is_property() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("is_available") {
        assert_eq!(
            g.nodes[&id].metadata.get("property").map(String::as_str),
            Some("true")
        );
    }
}

#[test]
fn method_qualified_names() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("add") {
        let qname = &g.nodes[&id].qualified_name;
        assert!(qname.contains("Store") && qname.contains("add"), "expected Store.add in qname");
    }
}

// ─── Parameters ───────────────────────────────────────────────────────────────

#[test]
fn store_add_has_item_parameter() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["add"];
    assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "item"));
}

#[test]
fn store_get_has_item_id_parameter() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["get"];
    assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "item_id"));
}

#[test]
fn self_parameter_not_emitted() {
    let g = extract();
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    assert!(!params.contains_key("self"), "self should not be a Parameter node");
}

// ─── Return types ─────────────────────────────────────────────────────────────

#[test]
fn discount_has_return_type() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    if let Some(&id) = fns.get("discount") {
        assert!(has_edge_containing(&g, id, &EdgeKind::Returns, "float"));
    }
}

// ─── Module-level functions ───────────────────────────────────────────────────

#[test]
fn function_discount_exists() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("discount"));
    let id = fns["discount"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn async_function_fetch_prices_exists() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("fetch_prices"));
    let id = fns["fetch_prices"];
    assert!(g.nodes[&id].is_async, "fetch_prices must be async");
}

#[test]
fn private_function_format_price() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("_format_price"));
    let id = fns["_format_price"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Private);
}

// ─── Calls in method bodies ───────────────────────────────────────────────────

#[test]
fn store_add_calls_repo_save() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["add"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Calls, "save"),
        "Store.add should call repo.save"
    );
}

#[test]
fn store_add_calls_logger_info() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["add"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Calls, "info"),
        "Store.add should call logger.info"
    );
}

#[test]
fn store_refresh_awaits_asyncio_sleep() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["refresh"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Awaits, "sleep")
            || has_edge_containing(&g, id, &EdgeKind::Calls, "sleep"),
        "refresh should await/call asyncio.sleep"
    );
}
