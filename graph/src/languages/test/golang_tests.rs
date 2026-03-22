use std::collections::HashMap;

use crate::graph::{DependencyGraph, EdgeKind, EdgeTarget, Language, NodeId, NodeKind, Visibility};
use crate::languages::golang::GoExtractor;
use crate::parser::LanguageExtractor;

// ─── Fixture ──────────────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/shop.go");
const FILE: &str    = "shop/shop.go";

fn extract() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    GoExtractor.extract(FIXTURE, FILE, &mut g);
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
    assert!(files.contains_key("shop.go"), "expected a File node named 'shop.go'");
}

#[test]
fn file_node_has_hash() {
    let g = extract();
    let file_node = g.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
    assert!(file_node.hash.is_some(), "File node must have a hash");
}

#[test]
fn file_language_is_go() {
    let g = extract();
    let file_node = g.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
    assert_eq!(file_node.language, Language::Go);
}

// ─── Package ──────────────────────────────────────────────────────────────────

#[test]
fn package_node_exists() {
    let g = extract();
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    assert!(pkgs.contains_key("shop"), "expected Package node 'shop'");
}

#[test]
fn package_is_public() {
    let g = extract();
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    let id = pkgs["shop"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

// ─── Imports ──────────────────────────────────────────────────────────────────

#[test]
fn imports_exist() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(!imports.is_empty(), "expected at least one import");
}

#[test]
fn stdlib_errors_import() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("errors"), "expected 'errors' import");
}

#[test]
fn stdlib_fmt_import() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("fmt"), "expected 'fmt' import");
}

#[test]
fn aliased_import_money() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    // Aliased as "money"
    assert!(imports.contains_key("money"), "expected aliased import 'money'");
    let id = imports["money"];
    assert_eq!(g.nodes[&id].metadata.get("alias").map(String::as_str), Some("money"));
}

// ─── Type alias ───────────────────────────────────────────────────────────────

#[test]
fn type_alias_price_exists() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    assert!(aliases.contains_key("Price"), "expected TypeAlias 'Price'");
}

#[test]
fn type_alias_price_is_public() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    let id = aliases["Price"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn named_type_status_exists() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    assert!(aliases.contains_key("Status"), "expected TypeAlias 'Status'");
}

// ─── Constants ────────────────────────────────────────────────────────────────

#[test]
fn constants_exist() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(!consts.is_empty(), "expected at least one constant");
}

#[test]
fn exported_constant_status_pending() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("StatusPending"), "expected 'StatusPending' constant");
    let id = consts["StatusPending"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn exported_constant_status_confirmed() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("StatusConfirmed"));
}

// ─── Variables ────────────────────────────────────────────────────────────────

#[test]
fn exported_global_variable() {
    let g = extract();
    let vars = nodes_of_kind(&g, &NodeKind::GlobalVariable);
    assert!(vars.contains_key("DefaultTimeout"), "expected 'DefaultTimeout' var");
    let id = vars["DefaultTimeout"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn unexported_global_variable() {
    let g = extract();
    let vars = nodes_of_kind(&g, &NodeKind::GlobalVariable);
    assert!(vars.contains_key("maxRetries"), "expected 'maxRetries' var");
    let id = vars["maxRetries"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Internal);
}

// ─── Interfaces ───────────────────────────────────────────────────────────────

#[test]
fn interface_repository_exists() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(ifaces.contains_key("Repository"), "expected Interface 'Repository'");
}

#[test]
fn interface_cached_repository_exists() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(ifaces.contains_key("CachedRepository"), "expected Interface 'CachedRepository'");
}

#[test]
fn interface_cached_extends_repository() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let id = ifaces["CachedRepository"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Extends, "Repository"),
        "CachedRepository should extend Repository"
    );
}

#[test]
fn repository_has_find_by_id_method() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let id = ifaces["Repository"];
    assert!(has_edge_to(&g, id, &EdgeKind::Contains, "FindByID"));
}

#[test]
fn repository_methods_are_abstract() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&mid) = methods.get("FindByID") {
        assert!(g.nodes[&mid].is_abstract, "interface methods must be abstract");
    }
}

// ─── Structs ──────────────────────────────────────────────────────────────────

#[test]
fn struct_item_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Item"), "expected Class 'Item'");
}

#[test]
fn struct_store_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Store"), "expected Class 'Store'");
}

#[test]
fn struct_base_store_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("BaseStore"), "expected Class 'BaseStore'");
}

#[test]
fn item_has_exported_fields() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Item"];
    assert!(has_edge_to(&g, id, &EdgeKind::Contains, "ID"));
    assert!(has_edge_to(&g, id, &EdgeKind::Contains, "Name"));
    assert!(has_edge_to(&g, id, &EdgeKind::Contains, "Price"));
    assert!(has_edge_to(&g, id, &EdgeKind::Contains, "Quantity"));
}

#[test]
fn item_has_unexported_field() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Item"];
    assert!(has_edge_to(&g, id, &EdgeKind::Contains, "tags"));
}

#[test]
fn exported_fields_are_public() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    if let Some(&id) = fields.get("ID") {
        assert_eq!(g.nodes[&id].visibility, Visibility::Public);
    }
}

#[test]
fn unexported_fields_are_internal() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    if let Some(&id) = fields.get("tags") {
        assert_eq!(g.nodes[&id].visibility, Visibility::Internal);
    }
}

#[test]
fn store_embeds_base_store() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Store"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Extends, "BaseStore"),
        "Store should embed (extend) BaseStore"
    );
}

// ─── Generics ─────────────────────────────────────────────────────────────────

#[test]
fn generic_struct_pair_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Pair"), "expected generic struct 'Pair'");
}

#[test]
fn pair_has_type_param_t() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Pair"];
    assert!(g.nodes[&id].generic_params.contains(&"T".to_owned()));
}

// ─── Functions ────────────────────────────────────────────────────────────────

#[test]
fn exported_function_new_store() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("NewStore"), "expected Function 'NewStore'");
    let id = fns["NewStore"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn unexported_function_new_item() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("newItem"), "expected Function 'newItem'");
    let id = fns["newItem"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Internal);
}

#[test]
fn exported_function_discount() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("Discount"));
}

#[test]
fn unexported_function_format_price() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("formatPrice"));
}

#[test]
fn new_store_has_parameters() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    let id = fns["NewStore"];
    assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "repo"));
    assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "name"));
}

#[test]
fn new_store_qualified_name() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    let id = fns["NewStore"];
    assert_eq!(g.nodes[&id].qualified_name, "shop.NewStore");
}

// ─── Methods ──────────────────────────────────────────────────────────────────

#[test]
fn method_add_exists() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("Add"), "expected Method 'Add'");
}

#[test]
fn method_get_exists() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("Get"), "expected Method 'Get'");
}

#[test]
fn method_name_exists() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("Name"), "expected Method 'Name' on BaseStore");
}

#[test]
fn method_add_receiver_is_store() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["Add"];
    assert_eq!(g.nodes[&id].metadata.get("receiver").map(String::as_str), Some("Store"));
}

#[test]
fn method_add_qualified_name() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["Add"];
    assert_eq!(g.nodes[&id].qualified_name, "shop.Store.Add");
}

#[test]
fn method_add_has_parameter() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["Add"];
    assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "item"));
}

#[test]
fn exported_method_is_public() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["Add"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

// ─── Call expressions ─────────────────────────────────────────────────────────

#[test]
fn method_add_calls_repo_save() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["Add"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Calls, "Save"),
        "Add should call repo.Save"
    );
}

#[test]
fn function_new_store_calls_errors_new() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    let id = fns["NewStore"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Calls, "New"),
        "NewStore should call errors.New"
    );
}

// ─── Composite literals (struct instantiation) ────────────────────────────────

#[test]
fn new_store_instantiates_store() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    let id = fns["NewStore"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Instantiates, "Store"),
        "NewStore should instantiate Store"
    );
}

#[test]
fn new_item_instantiates_item() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    let id = fns["newItem"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Instantiates, "Item"),
        "newItem should instantiate Item"
    );
}
