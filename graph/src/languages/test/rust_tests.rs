use std::collections::{HashMap, HashSet};

use crate::graph::{DependencyGraph, EdgeKind, EdgeTarget, Language, NodeId, NodeKind, Visibility};
use crate::languages::rust::RustExtractor;
use crate::parser::LanguageExtractor;

// ─── Fixture ──────────────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/shop.rs");
const FILE: &str = "src/shop.rs";

// ─── Helpers (same pattern as java_tests) ─────────────────────────────────────

fn extract() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    RustExtractor.extract(FIXTURE, FILE, &mut g);
    g.resolve();
    g
}

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

fn has_edge_to_name(g: &DependencyGraph, from: NodeId, kind: &EdgeKind, target: &str) -> bool {
    edges_from(g, from).into_iter().any(|e| {
        std::mem::discriminant(&e.kind) == std::mem::discriminant(kind)
            && match &e.to {
                EdgeTarget::Resolved(id) => g
                    .nodes
                    .get(id)
                    .map(|n| n.name == target || n.qualified_name == target)
                    .unwrap_or(false),
                EdgeTarget::Unresolved(s) | EdgeTarget::External(s) => {
                    s == target || s.ends_with(&format!("::{}", target))
                }
            }
    })
}

fn has_edge_containing(g: &DependencyGraph, from: NodeId, kind: &EdgeKind, substr: &str) -> bool {
    edges_from(g, from).into_iter().any(|e| {
        std::mem::discriminant(&e.kind) == std::mem::discriminant(kind)
            && match &e.to {
                EdgeTarget::Resolved(id) => g
                    .nodes
                    .get(id)
                    .map(|n| n.name.contains(substr) || n.qualified_name.contains(substr))
                    .unwrap_or(false),
                EdgeTarget::Unresolved(s) | EdgeTarget::External(s) => s.contains(substr),
            }
    })
}

fn all_nodes_named<'a>(g: &'a DependencyGraph, name: &str) -> Vec<&'a crate::graph::Node> {
    g.nodes.values().filter(|n| n.name == name).collect()
}

// ─── File node ────────────────────────────────────────────────────────────────

#[test]
fn test_file_node_created() {
    let g = extract();
    let files = nodes_of_kind(&g, &NodeKind::File);
    assert!(files.contains_key("shop.rs"), "expected File node named shop.rs");
    let fid = files["shop.rs"];
    assert_eq!(g.nodes[&fid].qualified_name, FILE);
    assert_eq!(g.nodes[&fid].language, Language::Rust);
}

// ─── use / imports ────────────────────────────────────────────────────────────

#[test]
fn test_simple_use_imported() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(
        imports.contains_key("HashMap"),
        "missing import HashMap; got: {:?}",
        imports.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_grouped_use_imported() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("Read"),      "missing import Read");
    assert!(imports.contains_key("Write"),     "missing import Write");
    assert!(imports.contains_key("Display"),   "missing import Display");
    assert!(imports.contains_key("Formatter"), "missing import Formatter");
}

#[test]
fn test_wildcard_use_flagged() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    let star_id = imports["*"];
    assert_eq!(
        g.nodes[&star_id].metadata.get("wildcard").map(String::as_str),
        Some("true"),
        "wildcard import should be flagged"
    );
}

#[test]
fn test_alias_use_recorded() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("SharedPtr"), "missing aliased import SharedPtr");
    let id = imports["SharedPtr"];
    assert_eq!(
        g.nodes[&id].metadata.get("alias").map(String::as_str),
        Some("SharedPtr")
    );
}

#[test]
fn test_pub_use_reexport_flagged() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    assert!(imports.contains_key("Ordering"), "missing pub use Ordering");
    let id = imports["Ordering"];
    assert_eq!(
        g.nodes[&id].metadata.get("reexport").map(String::as_str),
        Some("true")
    );
}

#[test]
fn test_pub_use_creates_reexports_edge() {
    let g = extract();
    let files = nodes_of_kind(&g, &NodeKind::File);
    let fid = files["shop.rs"];
    assert!(
        has_edge_containing(&g, fid, &EdgeKind::Reexports, "Ordering"),
        "file should have a Reexports edge for Ordering"
    );
}

#[test]
fn test_extern_crate_imported() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    // extern crate std as std_crate → simple name is "std_crate"
    assert!(
        imports.contains_key("std_crate") || imports.contains_key("std"),
        "extern crate std as std_crate should produce an Import node"
    );
}

// ─── mod ──────────────────────────────────────────────────────────────────────

#[test]
fn test_mod_extracted() {
    let g = extract();
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    assert!(pkgs.contains_key("pricing"), "missing mod 'pricing'");
    assert!(pkgs.contains_key("tests"),   "missing mod 'tests'");
}

#[test]
fn test_mod_contains_items() {
    let g = extract();
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    let pricing_id = pkgs["pricing"];
    assert!(
        has_edge_containing(&g, pricing_id, &EdgeKind::Contains, "apply_markup"),
        "pricing mod should Contain apply_markup"
    );
    assert!(
        has_edge_containing(&g, pricing_id, &EdgeKind::Contains, "BASE_MARKUP"),
        "pricing mod should Contain BASE_MARKUP"
    );
}

#[test]
fn test_mod_attributes_captured() {
    let g = extract();
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    let tests_id = pkgs["tests"];
    let attrs = &g.nodes[&tests_id].attributes;
    assert!(
        attrs.iter().any(|a| a.contains("cfg") && a.contains("test")),
        "tests mod should carry #[cfg(test)] attribute; got {:?}", attrs
    );
}

// ─── const and static ─────────────────────────────────────────────────────────

#[test]
fn test_top_level_const_extracted() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("MAX_STOCK"),       "missing const MAX_STOCK");
    assert!(consts.contains_key("DEFAULT_DISCOUNT"), "missing const DEFAULT_DISCOUNT");
}

#[test]
fn test_const_visibility() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    let max_id = consts["MAX_STOCK"];
    assert_eq!(g.nodes[&max_id].visibility, Visibility::Public);
    let def_id = consts["DEFAULT_DISCOUNT"];
    assert_eq!(g.nodes[&def_id].visibility, Visibility::Private);
}

#[test]
fn test_const_type_annotation() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    let id = consts["MAX_STOCK"];
    assert_eq!(
        g.nodes[&id].type_annotation.as_deref(), Some("u32"),
        "MAX_STOCK should have type u32"
    );
}

#[test]
fn test_static_extracted() {
    let g = extract();
    // immutable static → Constant; mutable static → StaticField
    let consts   = nodes_of_kind(&g, &NodeKind::Constant);
    let statics  = nodes_of_kind(&g, &NodeKind::StaticField);
    assert!(
        consts.contains_key("SHOP_NAME"),
        "immutable static SHOP_NAME should be a Constant"
    );
    assert!(
        statics.contains_key("GLOBAL_TAX_RATE"),
        "mutable static GLOBAL_TAX_RATE should be a StaticField"
    );
}

#[test]
fn test_mutable_static_flagged() {
    let g = extract();
    let statics = nodes_of_kind(&g, &NodeKind::StaticField);
    let id = statics["GLOBAL_TAX_RATE"];
    assert_eq!(
        g.nodes[&id].metadata.get("mutable").map(String::as_str),
        Some("true")
    );
}

// ─── type alias ───────────────────────────────────────────────────────────────

#[test]
fn test_type_alias_extracted() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    assert!(aliases.contains_key("ProductId"), "missing type alias ProductId");
    assert!(aliases.contains_key("Inventory"),  "missing type alias Inventory");
}

#[test]
fn test_type_alias_references_edge() {
    let g = extract();
    let aliases = nodes_of_kind(&g, &NodeKind::TypeAlias);
    let pid = aliases["ProductId"];
    assert!(
        has_edge_containing(&g, pid, &EdgeKind::References, "u64"),
        "ProductId should have a References edge to u64"
    );
}

// ─── Traits ───────────────────────────────────────────────────────────────────

#[test]
fn test_traits_extracted() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(ifaces.contains_key("Describable"),  "missing trait Describable");
    assert!(ifaces.contains_key("Priceable"),    "missing trait Priceable");
    assert!(ifaces.contains_key("Repository"),   "missing trait Repository");
}

#[test]
fn test_trait_visibility() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let id = ifaces["Describable"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn test_supertrait_extends_edge() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let pid = ifaces["Priceable"];
    assert!(
        has_edge_containing(&g, pid, &EdgeKind::Extends, "Describable"),
        "Priceable should Extend Describable"
    );
}

#[test]
fn test_trait_abstract_methods() {
    let g = extract();
    // Trait methods without body should be abstract.
    let desc_method = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method && n.name == "description" && n.is_abstract
    });
    assert!(desc_method, "Describable::description should be abstract");
}

#[test]
fn test_trait_default_method_not_abstract() {
    let g = extract();
    // `discounted_price` has a body → not abstract
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method && n.name == "discounted_price" && !n.is_abstract
    });
    assert!(found, "Priceable::discounted_price has a body and should not be abstract");
}

#[test]
fn test_trait_methods_contained() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let did = ifaces["Describable"];
    assert!(
        has_edge_containing(&g, did, &EdgeKind::Contains, "description"),
        "Describable should Contain description"
    );
}

// ─── Enums ────────────────────────────────────────────────────────────────────

#[test]
fn test_enums_extracted() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    assert!(enums.contains_key("Category"),  "missing enum Category");
    assert!(enums.contains_key("ShopError"), "missing enum ShopError");
}

#[test]
fn test_enum_visibility() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    let id = enums["Category"];
    assert_eq!(g.nodes[&id].visibility, Visibility::Public);
}

#[test]
fn test_enum_derive_attributes() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    let id = enums["Category"];
    let attrs = &g.nodes[&id].attributes;
    assert!(
        attrs.iter().any(|a| a.contains("derive") && a.contains("Debug")),
        "Category should have #[derive(Debug, ...)] attribute; got {:?}", attrs
    );
}

#[test]
fn test_enum_variants_extracted() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("Electronics"), "missing variant Electronics");
    assert!(consts.contains_key("Clothing"),    "missing variant Clothing");
    assert!(consts.contains_key("Food"),        "missing variant Food");
    assert!(consts.contains_key("Legacy"),      "missing variant Legacy");
}

#[test]
fn test_enum_variant_attributes() {
    let g = extract();
    // The `Legacy` variant carries #[deprecated]
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Constant
            && n.name == "Legacy"
            && n.attributes.iter().any(|a| a.contains("deprecated"))
    });
    assert!(found, "Legacy variant should carry #[deprecated]");
}

#[test]
fn test_enum_variants_contained() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    let cid = enums["Category"];
    assert!(
        has_edge_containing(&g, cid, &EdgeKind::Contains, "Electronics"),
        "Category should Contain Electronics"
    );
}

// ─── Structs ──────────────────────────────────────────────────────────────────

#[test]
fn test_structs_extracted() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Product"),      "missing struct Product");
    assert!(classes.contains_key("Discount"),     "missing struct Discount");
    assert!(classes.contains_key("Cart"),         "missing struct Cart");
    assert!(classes.contains_key("InMemoryRepo"), "missing struct InMemoryRepo");
}

#[test]
fn test_struct_visibility() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];
    assert_eq!(g.nodes[&pid].visibility, Visibility::Public);
}

#[test]
fn test_struct_derive_attributes() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];
    let attrs = &g.nodes[&pid].attributes;
    assert!(
        attrs.iter().any(|a| a.contains("derive") && a.contains("Clone")),
        "Product should carry #[derive(..., Clone, ...)]"
    );
}

#[test]
fn test_struct_generic_params() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let did = classes["Discount"];
    assert!(
        g.nodes[&did].generic_params.contains(&"T".to_owned()),
        "Discount should have generic param T"
    );
}

#[test]
fn test_struct_where_bounds() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let did = classes["Discount"];
    let bounds = &g.nodes[&did].generic_bounds;
    assert!(
        bounds.iter().any(|b| b.contains("Priceable")),
        "Discount should have where T: Priceable bound; got {:?}", bounds
    );
}

#[test]
fn test_struct_fields_extracted() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("id"),    "missing field id");
    assert!(fields.contains_key("name"),  "missing field name");
    assert!(fields.contains_key("price"), "missing field price");
    assert!(fields.contains_key("stock"), "missing field stock");
}

#[test]
fn test_field_visibility() {
    let g = extract();
    // `pub id` on Product
    let pub_field = g.nodes.values().any(|n| {
        n.kind == NodeKind::Field && n.name == "id" && n.visibility == Visibility::Public
    });
    assert!(pub_field, "Product::id should be Public");

    // `price` on Product (no pub modifier)
    let priv_field = g.nodes.values().any(|n| {
        n.kind == NodeKind::Field && n.name == "price" && n.visibility == Visibility::Private
    });
    assert!(priv_field, "Product::price should be Private");
}

#[test]
fn test_field_type_annotation() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    let id_fid = fields["id"];
    assert_eq!(
        g.nodes[&id_fid].type_annotation.as_deref(),
        Some("ProductId"),
        "field id should have type ProductId"
    );
}

#[test]
fn test_fields_contained_in_struct() {
    let g = extract();
    for node in g.nodes.values() {
        if node.kind == NodeKind::Field {
            let is_contained = g.edges.iter().any(|e| {
                matches!(e.kind, EdgeKind::Contains) && e.to == EdgeTarget::Resolved(node.id)
            });
            assert!(
                is_contained,
                "field '{}' (qname: {}) has no Contains edge",
                node.name, node.qualified_name
            );
        }
    }
}

// ─── impl / methods ───────────────────────────────────────────────────────────

#[test]
fn test_inherent_methods_extracted() {
    let g = extract();
    let methods: HashSet<_> = g.nodes.values()
        .filter(|n| n.kind == NodeKind::Method)
        .map(|n| n.name.as_str())
        .collect();
    for name in &["new", "restock", "price", "stock", "apply_tax", "fetch_metadata"] {
        assert!(methods.contains(name), "missing method '{}'", name);
    }
}

#[test]
fn test_method_visibility() {
    let g = extract();
    // `pub fn restock` → Public
    let pub_meth = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method && n.name == "restock" && n.visibility == Visibility::Public
    });
    assert!(pub_meth, "restock should be Public");

    // `fn apply_tax` (private) → Private
    let priv_meth = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method && n.name == "apply_tax" && n.visibility == Visibility::Private
    });
    assert!(priv_meth, "apply_tax should be Private");
}

#[test]
fn test_async_method_flagged() {
    let g = extract();
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method && n.name == "fetch_metadata" && n.is_async
    });
    assert!(found, "fetch_metadata should be flagged async");
}

#[test]
fn test_method_return_type() {
    let g = extract();
    let meth = g.nodes.values()
        .find(|n| n.kind == NodeKind::Method && n.name == "restock")
        .unwrap();
    // `pub fn restock(&mut self, quantity: u32)` returns ()  — no Returns edge expected
    // `pub fn price(&self) -> f64` should have Returns edge
    let price_meth = g.nodes.values()
        .find(|n| n.kind == NodeKind::Method && n.name == "price" && !n.is_abstract)
        .unwrap();
    assert!(
        has_edge_containing(&g, price_meth.id, &EdgeKind::Returns, "f64"),
        "price() should have Returns edge to f64"
    );
}

#[test]
fn test_impl_const_extracted() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(
        consts.contains_key("MINIMUM_PRICE"),
        "impl const MINIMUM_PRICE should be extracted"
    );
}

#[test]
fn test_trait_impl_creates_implements_edge() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];

    // Product has `impl Describable for Product` and `impl Priceable for Product`
    assert!(
        has_edge_containing(&g, pid, &EdgeKind::Implements, "Describable"),
        "Product should Implement Describable"
    );
    assert!(
        has_edge_containing(&g, pid, &EdgeKind::Implements, "Priceable"),
        "Product should Implement Priceable"
    );

    // ShopError (an enum) has `impl Display for ShopError`
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    let eid = enums["ShopError"];
    assert!(
        has_edge_containing(&g, eid, &EdgeKind::Implements, "Display"),
        "ShopError should Implement Display"
    );
}

#[test]
fn test_generic_struct_trait_impl() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let did = classes["Discount"];
    assert!(
        has_edge_containing(&g, did, &EdgeKind::Implements, "Describable"),
        "Discount should Implement Describable"
    );
    assert!(
        has_edge_containing(&g, did, &EdgeKind::Implements, "Priceable"),
        "Discount should Implement Priceable"
    );
}

#[test]
fn test_repo_impl_implements_edge() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let rid = classes["InMemoryRepo"];
    assert!(
        has_edge_containing(&g, rid, &EdgeKind::Implements, "Repository"),
        "InMemoryRepo should Implement Repository"
    );
}

// ─── Method parameters ────────────────────────────────────────────────────────

#[test]
fn test_parameters_extracted() {
    let g = extract();
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    assert!(params.contains_key("quantity"), "missing parameter 'quantity'");
    assert!(params.contains_key("discount"),  "missing parameter 'discount'");
}

#[test]
fn test_parameter_type_annotation() {
    let g = extract();
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    let qid = params["quantity"];
    assert_eq!(
        g.nodes[&qid].type_annotation.as_deref(), Some("u32"),
        "parameter 'quantity' should have type u32"
    );
}

#[test]
fn test_self_not_a_parameter_node() {
    let g = extract();
    // `self`, `&self`, `&mut self` should NOT create Parameter nodes.
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    assert!(
        !params.contains_key("self"),
        "self receiver should not be a Parameter node"
    );
}

// ─── Top-level functions ──────────────────────────────────────────────────────

#[test]
fn test_top_level_functions_extracted() {
    let g = extract();
    let funcs = nodes_of_kind(&g, &NodeKind::Function);
    assert!(funcs.contains_key("print_inventory"),     "missing fn print_inventory");
    assert!(funcs.contains_key("build_sample_inventory"), "missing fn build_sample_inventory");
    assert!(funcs.contains_key("cheapest"),            "missing fn cheapest");
    assert!(funcs.contains_key("fetch_all_metadata"),  "missing fn fetch_all_metadata");
}

#[test]
fn test_async_function_flagged() {
    let g = extract();
    let funcs = nodes_of_kind(&g, &NodeKind::Function);
    let fid = funcs["fetch_all_metadata"];
    assert!(g.nodes[&fid].is_async, "fetch_all_metadata should be async");
}

#[test]
fn test_generic_function_params() {
    let g = extract();
    let funcs = nodes_of_kind(&g, &NodeKind::Function);
    let cid = funcs["cheapest"];
    assert!(
        g.nodes[&cid].generic_params.contains(&"T".to_owned()),
        "cheapest should have generic param T"
    );
}

#[test]
fn test_generic_function_where_bounds() {
    let g = extract();
    let funcs = nodes_of_kind(&g, &NodeKind::Function);
    let cid = funcs["cheapest"];
    let bounds = &g.nodes[&cid].generic_bounds;
    assert!(
        bounds.iter().any(|b| b.contains("Priceable")),
        "cheapest should have where T: Priceable bound; got {:?}", bounds
    );
}

// ─── Function body edges ──────────────────────────────────────────────────────

#[test]
fn test_local_variable_extracted() {
    let g = extract();
    let vars = nodes_of_kind(&g, &NodeKind::Variable);
    assert!(
        vars.contains_key("items") || vars.contains_key("inv") || vars.contains_key("factor"),
        "expected some local variables; got {:?}", vars.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_let_with_type_annotation() {
    let g = extract();
    // `let entry: (Product, u32) = ...` in Cart::add
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Variable
            && n.name == "entry"
            && n.type_annotation.is_some()
    });
    assert!(found, "local variable 'entry' should have a type annotation");
}

#[test]
fn test_writes_edge_from_assignment() {
    let g = extract();
    // `self.stock = self.stock + quantity` in restock → Writes edge to "stock"
    let restock = g.nodes.values()
        .find(|n| n.kind == NodeKind::Method && n.name == "restock")
        .unwrap();
    assert!(
        has_edge_containing(&g, restock.id, &EdgeKind::Writes, "stock"),
        "restock should have a Writes edge to stock"
    );
}

#[test]
fn test_method_call_edge() {
    let g = extract();
    // Cart::add calls self.items.push(entry)
    let add_meth = g.nodes.values()
        .find(|n| n.kind == NodeKind::Method && n.name == "add")
        .unwrap();
    assert!(
        has_edge_containing(&g, add_meth.id, &EdgeKind::Calls, "push"),
        "Cart::add should have a Calls edge to push"
    );
}

#[test]
fn test_function_call_edge() {
    let g = extract();
    // Cart::total calls pricing::round_to_cents
    let total = g.nodes.values()
        .find(|n| (n.kind == NodeKind::Method || n.kind == NodeKind::Function) && n.name == "total")
        .unwrap();
    assert!(
        has_edge_containing(&g, total.id, &EdgeKind::Calls, "round_to_cents"),
        "Cart::total should call round_to_cents"
    );
}

#[test]
fn test_macro_call_edge() {
    let g = extract();
    // `println!` used in print_inventory
    let func = g.nodes.values()
        .find(|n| n.name == "print_inventory")
        .unwrap();
    assert!(
        has_edge_containing(&g, func.id, &EdgeKind::Calls, "println!"),
        "print_inventory should have Calls edge to println!"
    );
}

#[test]
fn test_struct_instantiation_edge() {
    let g = extract();
    // Cart::new instantiates Cart { items, owner }
    let cart_new = g.nodes.values()
        .find(|n| n.kind == NodeKind::Method && n.name == "new"
              && n.qualified_name.contains("Cart"))
        .unwrap();
    assert!(
        has_edge_containing(&g, cart_new.id, &EdgeKind::Instantiates, "Cart"),
        "Cart::new should Instantiate Cart"
    );
}

#[test]
fn test_closure_node_extracted() {
    let g = extract();
    let closures = nodes_of_kind(&g, &NodeKind::Closure);
    assert!(
        !closures.is_empty(),
        "expected at least one Closure node (from Cart::total or test closures)"
    );
}

#[test]
fn test_closure_contained_in_function() {
    let g = extract();
    // The `discount_fn` closure in Cart::total should be contained in total
    let total = g.nodes.values()
        .find(|n| n.name == "total")
        .unwrap();
    assert!(
        has_edge_containing(&g, total.id, &EdgeKind::Contains, "<closure>"),
        "total should Contain a closure"
    );
}

#[test]
fn test_await_edge_recorded() {
    let g = extract();
    // fetch_all_metadata awaits p.fetch_metadata(...)
    let func = g.nodes.values()
        .find(|n| n.name == "fetch_all_metadata")
        .unwrap();
    assert!(
        has_edge_containing(&g, func.id, &EdgeKind::Awaits, "fetch_metadata"),
        "fetch_all_metadata should have an Awaits edge to fetch_metadata"
    );
}

#[test]
fn test_call_arity_recorded() {
    let g = extract();
    let calls: Vec<_> = g.edges.iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls) && e.call_arity.is_some())
        .collect();
    assert!(!calls.is_empty(), "Calls edges should record arity");
}

// ─── Resolution pass ──────────────────────────────────────────────────────────

#[test]
fn test_contains_edges_all_resolved() {
    let g = extract();
    let unresolved_contains = g.edges.iter().filter(|e| {
        matches!(e.kind, EdgeKind::Contains) && matches!(e.to, EdgeTarget::Unresolved(_))
    }).count();
    assert_eq!(unresolved_contains, 0, "all Contains edges must be resolved");
}

#[test]
fn test_has_resolved_edges() {
    let g = extract();
    let resolved = g.edges.iter().filter(|e| matches!(e.to, EdgeTarget::Resolved(_))).count();
    assert!(resolved > 0, "no resolved edges — resolution pass did not run");
}

// ─── Structural invariants ────────────────────────────────────────────────────

#[test]
fn test_all_methods_contained() {
    let g = extract();
    for node in g.nodes.values() {
        if node.kind == NodeKind::Method {
            let contained = g.edges.iter().any(|e| {
                matches!(e.kind, EdgeKind::Contains) && e.to == EdgeTarget::Resolved(node.id)
            });
            assert!(
                contained,
                "method '{}' (qname: {}) has no Contains edge",
                node.name, node.qualified_name
            );
        }
    }
}

#[test]
fn test_node_counts_reasonable() {
    let g = extract();
    assert!(g.node_count() > 60, "expected >60 nodes, got {}", g.node_count());
    assert!(g.edge_count() > 80, "expected >80 edges, got {}", g.edge_count());
}

// ─── hash / description ───────────────────────────────────────────────────────

#[test]
fn test_file_node_has_hash() {
    let g = extract();
    let file_node = g.nodes.values().find(|n| n.kind == NodeKind::File)
        .expect("no File node found");
    assert!(file_node.hash.is_some(), "File node should have a non-None hash");
}

#[test]
fn test_file_hash_is_sha256_hex() {
    let g = extract();
    let file_node = g.nodes.values().find(|n| n.kind == NodeKind::File)
        .expect("no File node found");
    let hash = file_node.hash.as_deref().unwrap();
    assert_eq!(hash.len(), 64, "SHA-256 hex digest should be 64 characters");
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "hash should be lowercase hex");
}

#[test]
fn test_file_hash_is_deterministic() {
    let g1 = extract();
    let g2 = extract();
    let h1 = g1.nodes.values().find(|n| n.kind == NodeKind::File).unwrap().hash.clone();
    let h2 = g2.nodes.values().find(|n| n.kind == NodeKind::File).unwrap().hash.clone();
    assert_eq!(h1, h2, "same source should always produce the same hash");
}

#[test]
fn test_non_file_nodes_have_no_hash() {
    let g = extract();
    for node in g.nodes.values() {
        if node.kind != NodeKind::File {
            assert!(
                node.hash.is_none(),
                "non-File node '{}' ({:?}) should have hash = None",
                node.name, node.kind
            );
        }
    }
}

#[test]
fn test_description_defaults_to_none() {
    let g = extract();
    for node in g.nodes.values() {
        assert!(
            node.description.is_none(),
            "node '{}' ({:?}) should have description = None (no doc-comment extraction yet)",
            node.name, node.kind
        );
    }
}
