use crate::graph::DependencyGraph;

/// Errors that can arise during serialization or deserialization.
#[derive(Debug)]
pub enum SerializerError {
    Serialize(serde_yaml::Error),
    Deserialize(serde_yaml::Error),
}

impl std::fmt::Display for SerializerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializerError::Serialize(e)   => write!(f, "serialization error: {}", e),
            SerializerError::Deserialize(e) => write!(f, "deserialization error: {}", e),
        }
    }
}

pub struct GraphSerializer;

impl GraphSerializer {
    /// Serialize a [`DependencyGraph`] to a YAML string.
    pub fn serialize(graph: &DependencyGraph) -> Result<String, SerializerError> {
        serde_yaml::to_string(graph).map_err(SerializerError::Serialize)
    }

    /// Deserialize a [`DependencyGraph`] from a YAML string.
    pub fn deserialize(yaml: &str) -> Result<DependencyGraph, SerializerError> {
        serde_yaml::from_str(yaml).map_err(SerializerError::Deserialize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{DependencyGraph, EdgeKind, EdgeTarget, Language, Node, NodeKind, Span, Visibility};

    fn sample_graph() -> DependencyGraph {
        let mut g = DependencyGraph::new();

        let mut file = Node::new(
            0, NodeKind::File,
            "Main.java", "com/example/Main.java",
            "com/example/Main.java",
            Span::new(0, 0, 0, 0),
            Language::Java,
        );
        file.visibility = Visibility::Public;
        let file_id = g.add_node(file);

        let mut cls = Node::new(
            0, NodeKind::Class,
            "Main", "com.example.Main",
            "com/example/Main.java",
            Span::new(1, 0, 50, 1),
            Language::Java,
        );
        cls.visibility = Visibility::Public;
        cls.generic_params = vec!["T".into()];
        cls.attributes = vec!["@SuppressWarnings".into()];
        let cls_id = g.add_node(cls);

        g.add_edge_simple(
            EdgeKind::Contains,
            file_id,
            EdgeTarget::Resolved(cls_id),
            Span::new(1, 0, 50, 1),
        );

        let mut method = Node::new(
            0, NodeKind::Method,
            "run", "com.example.Main.run",
            "com/example/Main.java",
            Span::new(5, 2, 20, 3),
            Language::Java,
        );
        method.visibility   = Visibility::Public;
        method.is_async     = true;
        method.type_annotation = Some("void".into());
        let method_id = g.add_node(method);

        g.add_edge_simple(
            EdgeKind::Contains,
            cls_id,
            EdgeTarget::Resolved(method_id),
            Span::new(5, 2, 20, 3),
        );
        g.add_edge_simple(
            EdgeKind::Calls,
            method_id,
            EdgeTarget::Unresolved("helper".into()),
            Span::new(10, 4, 10, 18),
        );

        g.resolve();
        g
    }

    // ── round-trip ────────────────────────────────────────────────────────────

    #[test]
    fn test_serialize_produces_yaml() {
        let g = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialization failed");
        assert!(!yaml.is_empty(), "serialized YAML should not be empty");
        assert!(yaml.contains("nodes:"), "YAML should contain a 'nodes' key");
        assert!(yaml.contains("edges:"), "YAML should contain an 'edges' key");
    }

    #[test]
    fn test_round_trip_node_count() {
        let g = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");
        assert_eq!(g.node_count(), g2.node_count(), "node count must survive round-trip");
    }

    #[test]
    fn test_round_trip_edge_count() {
        let g = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");
        assert_eq!(g.edge_count(), g2.edge_count(), "edge count must survive round-trip");
    }

    #[test]
    fn test_round_trip_node_fields() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");

        for (id, original) in &g.nodes {
            let restored = g2.nodes.get(id)
                .unwrap_or_else(|| panic!("node {} missing after round-trip", id));

            assert_eq!(original.name,           restored.name);
            assert_eq!(original.qualified_name, restored.qualified_name);
            assert_eq!(original.kind,           restored.kind);
            assert_eq!(original.language,       restored.language);
            assert_eq!(original.visibility,     restored.visibility);
            assert_eq!(original.is_async,       restored.is_async);
            assert_eq!(original.type_annotation, restored.type_annotation);
            assert_eq!(original.generic_params, restored.generic_params);
            assert_eq!(original.attributes,     restored.attributes);
            assert_eq!(original.span,           restored.span);
        }
    }

    #[test]
    fn test_round_trip_edge_fields() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");

        assert_eq!(g.edges.len(), g2.edges.len());
        for (orig, rest) in g.edges.iter().zip(g2.edges.iter()) {
            assert_eq!(orig.kind,        rest.kind);
            assert_eq!(orig.from,        rest.from);
            assert_eq!(orig.to,          rest.to);
            assert_eq!(orig.span,        rest.span);
            assert_eq!(orig.call_arity,  rest.call_arity);
        }
    }

    #[test]
    fn test_round_trip_indices() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");

        // by_qualified index must survive
        for (qname, &id) in &g.by_qualified {
            let restored_id = g2.by_qualified.get(qname)
                .unwrap_or_else(|| panic!("qualified name '{}' missing after round-trip", qname));
            assert_eq!(id, *restored_id);
        }

        // by_file index must survive
        for (file, ids) in &g.by_file {
            let restored_ids = g2.by_file.get(file)
                .unwrap_or_else(|| panic!("file '{}' missing after round-trip", file));
            let a: std::collections::HashSet<_> = ids.iter().collect();
            let b: std::collections::HashSet<_> = restored_ids.iter().collect();
            assert_eq!(a, b, "by_file entry for '{}' differs after round-trip", file);
        }
    }

    #[test]
    fn test_round_trip_edge_targets() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");

        let resolved_orig: Vec<_> = g.edges.iter()
            .filter(|e| matches!(e.to, EdgeTarget::Resolved(_)))
            .collect();
        let resolved_rest: Vec<_> = g2.edges.iter()
            .filter(|e| matches!(e.to, EdgeTarget::Resolved(_)))
            .collect();
        assert_eq!(resolved_orig.len(), resolved_rest.len(),
            "resolved edge count must survive round-trip");

        let unresolved_orig: Vec<_> = g.edges.iter()
            .filter(|e| matches!(e.to, EdgeTarget::Unresolved(_)))
            .collect();
        let unresolved_rest: Vec<_> = g2.edges.iter()
            .filter(|e| matches!(e.to, EdgeTarget::Unresolved(_)))
            .collect();
        assert_eq!(unresolved_orig.len(), unresolved_rest.len(),
            "unresolved edge count must survive round-trip");
    }

    #[test]
    fn test_round_trip_next_id_counters() {
        // After a round-trip, adding a new node must not collide with existing ids.
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let mut g2 = GraphSerializer::deserialize(&yaml).expect("deserialize");

        let existing_ids: std::collections::HashSet<u64> = g2.nodes.keys().cloned().collect();
        let new_node = Node::new(
            0, NodeKind::Function, "newFn", "newFn",
            "extra.java", Span::new(0, 0, 1, 0), Language::Java,
        );
        let new_id = g2.add_node(new_node);
        assert!(
            !existing_ids.contains(&new_id),
            "new node id {} collides with existing ids after round-trip", new_id
        );
    }

    // ── error handling ────────────────────────────────────────────────────────

    #[test]
    fn test_deserialize_invalid_yaml_returns_error() {
        let result = GraphSerializer::deserialize("not: valid: yaml: [[[");
        assert!(result.is_err(), "invalid YAML should return an error");
    }

    #[test]
    fn test_deserialize_wrong_schema_returns_error() {
        let result = GraphSerializer::deserialize("foo: bar\nbaz: 42\n");
        assert!(result.is_err(), "YAML with wrong schema should return an error");
    }

    // ── YAML content checks ───────────────────────────────────────────────────

    #[test]
    fn test_yaml_contains_node_names() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        assert!(yaml.contains("Main"),       "YAML should contain class name 'Main'");
        assert!(yaml.contains("run"),        "YAML should contain method name 'run'");
        assert!(yaml.contains("Main.java"),  "YAML should contain file name");
    }

    #[test]
    fn test_yaml_contains_edge_kinds() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        assert!(yaml.contains("Contains"), "YAML should contain edge kind 'Contains'");
        assert!(yaml.contains("Calls"),    "YAML should contain edge kind 'Calls'");
    }

    #[test]
    fn test_yaml_contains_language() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        assert!(yaml.contains("Java"), "YAML should contain language 'Java'");
    }

    #[test]
    fn test_yaml_contains_visibility() {
        let g    = sample_graph();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        assert!(yaml.contains("Public"), "YAML should contain visibility 'Public'");
    }

    // ── description / hash ────────────────────────────────────────────────────

    #[test]
    fn test_round_trip_description_and_hash() {
        let mut g = DependencyGraph::new();

        let mut file = Node::new(
            0, NodeKind::File,
            "Main.java", "com/example/Main.java",
            "com/example/Main.java",
            Span::new(0, 0, 0, 0),
            Language::Java,
        );
        file.hash = Some("a".repeat(64));
        file.description = None;
        let file_id = g.add_node(file);

        let mut cls = Node::new(
            0, NodeKind::Class,
            "Main", "com.example.Main",
            "com/example/Main.java",
            Span::new(1, 0, 10, 1),
            Language::Java,
        );
        cls.description = Some("The main entry-point class.".into());
        let cls_id = g.add_node(cls);

        g.add_edge_simple(
            EdgeKind::Contains,
            file_id,
            EdgeTarget::Resolved(cls_id),
            Span::new(1, 0, 10, 1),
        );
        g.resolve();

        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");

        let file2 = g2.nodes.values().find(|n| n.kind == NodeKind::File).unwrap();
        assert_eq!(file2.hash.as_deref(), Some(&*"a".repeat(64)),
            "File hash should survive round-trip");
        assert!(file2.description.is_none(), "File description should remain None");

        let cls2 = g2.nodes.values().find(|n| n.kind == NodeKind::Class).unwrap();
        assert!(cls2.hash.is_none(), "Class hash should remain None");
        assert_eq!(cls2.description.as_deref(), Some("The main entry-point class."),
            "Class description should survive round-trip");
    }

    #[test]
    fn test_yaml_contains_description_value() {
        let mut g = DependencyGraph::new();
        let mut cls = Node::new(
            0, NodeKind::Class,
            "Widget", "com.example.Widget",
            "Widget.java",
            Span::new(1, 0, 5, 1),
            Language::Java,
        );
        cls.description = Some("A reusable UI widget.".into());
        g.add_node(cls);

        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        assert!(yaml.contains("A reusable UI widget."),
            "YAML should contain the description string");
    }

    #[test]
    fn test_yaml_contains_hash_value() {
        let mut g = DependencyGraph::new();
        let mut file = Node::new(
            0, NodeKind::File,
            "Widget.java", "Widget.java",
            "Widget.java",
            Span::new(0, 0, 0, 0),
            Language::Java,
        );
        file.hash = Some("deadbeef".repeat(8));
        g.add_node(file);

        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        assert!(yaml.contains(&"deadbeef".repeat(8)),
            "YAML should contain the hash value");
    }

    // ── empty graph ───────────────────────────────────────────────────────────

    #[test]
    fn test_empty_graph_round_trip() {
        let g    = DependencyGraph::new();
        let yaml = GraphSerializer::serialize(&g).expect("serialize");
        let g2   = GraphSerializer::deserialize(&yaml).expect("deserialize");
        assert_eq!(g2.node_count(), 0);
        assert_eq!(g2.edge_count(), 0);
    }
}
