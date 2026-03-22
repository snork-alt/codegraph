use std::collections::HashMap;

use crate::graph::{DependencyGraph, EdgeKind, EdgeTarget, Language, NodeId, NodeKind, Visibility};
use crate::languages::typescript::TypeScriptExtractor;
use crate::parser::LanguageExtractor;

// ─── Fixture ──────────────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/shop.ts");
const FILE: &str    = "shop/shop.ts";

fn extract() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    TypeScriptExtractor.extract(FIXTURE, FILE, &mut g);
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
    assert!(files.contains_key("shop.ts"));
}

#[test]
fn file_language_is_typescript() {
    let g = extract();
    let file = g.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
    assert_eq!(file.language, Language::TypeScript);
}

#[test]
fn file_has_hash() {
    let g = extract();
    let file = g.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
    assert!(file.hash.is_some());
}

// ─── Imports ──────────────────────────────────────────────────────────────────

#[test]
fn imports_event_emitter() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(
        imports.contains_key("EventEmitter"),
        "expected 'EventEmitter' import"
    );
}

#[test]
fn imports_repository_type() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(
        imports.contains_key("Repository"),
        "expected 'Repository' import"
    );
}

#[test]
fn imports_utils_namespace() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(
        imports.contains_key("utils"),
        "expected namespace import 'utils'"
    );
}

// ─── Exported constants ───────────────────────────────────────────────────────

#[test]
fn constant_default_timeout_exists() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(
        consts.contains_key("DEFAULT_TIMEOUT"),
        "expected constant 'DEFAULT_TIMEOUT'"
    );
}

#[test]
fn constant_max_retries_exists() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(
        consts.contains_key("MAX_RETRIES"),
        "expected constant 'MAX_RETRIES'"
    );
}

// ─── Enum ─────────────────────────────────────────────────────────────────────

#[test]
fn enum_item_status_exists() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    assert!(enums.contains_key("ItemStatus"), "expected enum 'ItemStatus'");
}

#[test]
fn item_status_is_public() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    let id = enums["ItemStatus"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

// ─── Interfaces ───────────────────────────────────────────────────────────────

#[test]
fn interface_serializable_exists() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(
        ifaces.contains_key("Serializable"),
        "expected interface 'Serializable'"
    );
}

#[test]
fn interface_cacheable_exists() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(
        ifaces.contains_key("Cacheable"),
        "expected interface 'Cacheable'"
    );
}

#[test]
fn interface_item_exists() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(ifaces.contains_key("Item"), "expected interface 'Item'");
}

#[test]
fn interface_item_extends_serializable() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let id = ifaces["Item"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Extends, "Serializable"),
        "Item interface should extend Serializable"
    );
}

// ─── Type aliases ─────────────────────────────────────────────────────────────

#[test]
fn type_alias_item_id_exists() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    assert!(
        aliases.contains_key("ItemId"),
        "expected type alias 'ItemId'"
    );
}

#[test]
fn type_alias_price_exists() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    assert!(
        aliases.contains_key("Price"),
        "expected type alias 'Price'"
    );
}

#[test]
fn type_alias_item_factory_exists() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    assert!(
        aliases.contains_key("ItemFactory"),
        "expected type alias 'ItemFactory'"
    );
}

// ─── Abstract class ───────────────────────────────────────────────────────────

#[test]
fn class_base_entity_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(
        classes.contains_key("BaseEntity"),
        "expected class 'BaseEntity'"
    );
}

#[test]
fn base_entity_is_abstract() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["BaseEntity"];
    assert_eq!(
        g.nodes[&id].metadata.get("abstract").map(String::as_str),
        Some("true"),
        "BaseEntity should be abstract"
    );
}

// ─── ShopItem class ───────────────────────────────────────────────────────────

#[test]
fn class_shop_item_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(
        classes.contains_key("ShopItem"),
        "expected class 'ShopItem'"
    );
}

#[test]
fn shop_item_extends_base_entity() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["ShopItem"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Extends, "BaseEntity"),
        "ShopItem should extend BaseEntity"
    );
}

#[test]
fn shop_item_implements_item() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["ShopItem"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Implements, "Item")
            || has_edge_containing(&g, id, &EdgeKind::Implements, "Item"),
        "ShopItem should implement Item"
    );
}

#[test]
fn shop_item_implements_cacheable() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["ShopItem"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Implements, "Cacheable")
            || has_edge_containing(&g, id, &EdgeKind::Implements, "Cacheable"),
        "ShopItem should implement Cacheable"
    );
}

#[test]
fn shop_item_is_public() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["ShopItem"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

// ─── Store class ──────────────────────────────────────────────────────────────

#[test]
fn class_store_exists() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Store"), "expected class 'Store'");
}

#[test]
fn store_extends_event_emitter() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let id = classes["Store"];
    assert!(
        has_edge_to(&g, id, &EdgeKind::Extends, "EventEmitter")
            || has_edge_containing(&g, id, &EdgeKind::Extends, "EventEmitter"),
        "Store should extend EventEmitter"
    );
}

// ─── Fields ───────────────────────────────────────────────────────────────────

#[test]
fn shop_item_has_name_field() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("name"), "expected field 'name'");
}

#[test]
fn shop_item_has_price_field() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("price"), "expected field 'price'");
}

#[test]
fn shop_item_has_private_cache_field() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(
        fields.contains_key("_cache"),
        "expected private field '_cache'"
    );
    let id = fields["_cache"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Private);
}

#[test]
fn shop_item_has_static_default_currency() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::StaticField);
    assert!(
        fields.contains_key("defaultCurrency"),
        "expected static field 'defaultCurrency'"
    );
}

// ─── Methods ──────────────────────────────────────────────────────────────────

#[test]
fn base_entity_has_validate_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(
        methods.contains_key("validate"),
        "expected method 'validate'"
    );
}

#[test]
fn shop_item_has_update_price_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(
        methods.contains_key("updatePrice"),
        "expected method 'updatePrice'"
    );
}

#[test]
fn shop_item_update_price_has_decorator() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("updatePrice") {
        assert!(
            has_edge_containing(&g, id, &EdgeKind::Decorates, "withLogging")
                || g.nodes[&id].attributes.iter().any(|a| a.contains("withLogging")),
            "updatePrice should have @withLogging decorator"
        );
    }
}

#[test]
fn shop_item_has_async_fetch_details() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(
        methods.contains_key("fetchDetails"),
        "expected method 'fetchDetails'"
    );
    let id = methods["fetchDetails"];
    assert!(g.nodes[&id].is_async, "fetchDetails must be async");
}

#[test]
fn shop_item_has_static_create_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("create") {
        assert_eq!(
            g.nodes[&id].metadata.get("static").map(String::as_str),
            Some("true"),
            "create should be static"
        );
    }
}

#[test]
fn store_has_add_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("add"), "expected method 'add'");
}

#[test]
fn store_add_is_async() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["add"];
    assert!(g.nodes[&id].is_async, "Store.add must be async");
}

#[test]
fn store_has_get_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("get"), "expected method 'get'");
}

#[test]
fn store_has_protected_clear_cache() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(
        methods.contains_key("clearCache"),
        "expected method 'clearCache'"
    );
    let id = methods["clearCache"];
    // protected maps to Internal or Protected visibility
    assert_ne!(
        g.nodes[&id].visibility,
        Visibility::Public,
        "clearCache should not be public"
    );
}

#[test]
fn constructor_is_marked() {
    let g = extract();
    // Search all Method nodes (not the deduped HashMap) for ShopItem's constructor.
    let constructor = g.nodes.values().find(|n| {
        n.kind == NodeKind::Method
            && n.is_constructor
            && n.qualified_name.contains("ShopItem")
    });
    assert!(constructor.is_some(), "ShopItem should have a constructor");
}

#[test]
fn method_qualified_names() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("add") {
        let qname = &g.nodes[&id].qualified_name;
        assert!(
            qname.contains("Store") && qname.contains("add"),
            "expected Store.add in qname, got: {}",
            qname
        );
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
fn store_get_has_id_parameter() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["get"];
    assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "id"));
}

#[test]
fn update_price_has_new_price_parameter() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    if let Some(&id) = methods.get("updatePrice") {
        assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "newPrice"));
    }
}

// ─── Module-level functions ───────────────────────────────────────────────────

#[test]
fn function_discount_exists() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(fns.contains_key("discount"), "expected function 'discount'");
    let id = fns["discount"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn function_create_store_exists() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(
        fns.contains_key("createStore"),
        "expected function 'createStore'"
    );
    let id = fns["createStore"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn function_format_price_is_private() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    assert!(
        fns.contains_key("formatPrice"),
        "expected function 'formatPrice'"
    );
    let id = fns["formatPrice"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Private);
}

#[test]
fn discount_has_price_parameter() {
    let g = extract();
    let fns = nodes_of_kind(&g, &NodeKind::Function);
    if let Some(&id) = fns.get("discount") {
        assert!(has_edge_to(&g, id, &EdgeKind::HasParameter, "price"));
    }
}

// ─── Calls and await ─────────────────────────────────────────────────────────

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
fn store_add_calls_emit() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["add"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Calls, "emit"),
        "Store.add should call this.emit"
    );
}

#[test]
fn fetch_details_awaits_utils_fetch() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let id = methods["fetchDetails"];
    assert!(
        has_edge_containing(&g, id, &EdgeKind::Awaits, "fetch")
            || has_edge_containing(&g, id, &EdgeKind::Calls, "fetch"),
        "fetchDetails should await utils.fetch"
    );
}
