use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::{
    DependencyGraph, Edge, EdgeKind, EdgeTarget, Node, NodeId, NodeKind, Visibility,
};

// ─── Public result types ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Summary {
    pub total_nodes: usize,
    pub total_edges: usize,
    /// Node counts keyed by `NodeKind` debug name.
    pub node_counts: HashMap<String, usize>,
    /// Edge counts keyed by `EdgeKind` debug name.
    pub edge_counts: HashMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDependency {
    pub from: String,
    pub to: String,
    pub edge_count: usize,
}

#[derive(Debug, Clone)]
pub struct LayerViolation {
    pub from_node: NodeId,
    pub to_node: NodeId,
    /// Index of the "from" node's layer in the layers slice.
    pub from_layer: usize,
    /// Index of the "to" node's layer in the layers slice (must be < from_layer to be a violation).
    pub to_layer: usize,
}

// ─── Indexes ──────────────────────────────────────────────────────────────────

/// Pre-computed adjacency indexes over resolved edges.
struct Indexes {
    // Contains hierarchy
    contains_children: HashMap<NodeId, Vec<NodeId>>,
    contains_parent:   HashMap<NodeId, NodeId>,

    // Calls: caller→callees, callee→callers
    calls_out: HashMap<NodeId, Vec<NodeId>>,
    calls_in:  HashMap<NodeId, Vec<NodeId>>,

    // Implements: type→interfaces, interface→implementors
    implements_out: HashMap<NodeId, Vec<NodeId>>,
    implements_in:  HashMap<NodeId, Vec<NodeId>>,

    // Extends: type→parents, parent→children
    extends_out: HashMap<NodeId, Vec<NodeId>>,
    extends_in:  HashMap<NodeId, Vec<NodeId>>,

    // Overrides: method→overridden, overridden→overriders
    overrides_out: HashMap<NodeId, Vec<NodeId>>,
    overrides_in:  HashMap<NodeId, Vec<NodeId>>,

    // Reads: method→fields, field→readers
    reads_out: HashMap<NodeId, Vec<NodeId>>,
    reads_in:  HashMap<NodeId, Vec<NodeId>>,

    // Writes: method→fields, field→writers
    writes_out: HashMap<NodeId, Vec<NodeId>>,
    writes_in:  HashMap<NodeId, Vec<NodeId>>,

    // Imports: node→imported, imported→importers
    imports_out: HashMap<NodeId, Vec<NodeId>>,
    imports_in:  HashMap<NodeId, Vec<NodeId>>,

    // Instantiates: method→class, class→instantiators
    instantiates_out: HashMap<NodeId, Vec<NodeId>>,
    instantiates_in:  HashMap<NodeId, Vec<NodeId>>,

    // HasType: node→type, type→typed-nodes
    has_type_out: HashMap<NodeId, Vec<NodeId>>,
    has_type_in:  HashMap<NodeId, Vec<NodeId>>,

    // Returns: fn→return-type, type→returning-fns
    returns_out: HashMap<NodeId, Vec<NodeId>>,
    returns_in:  HashMap<NodeId, Vec<NodeId>>,

    // Throws: method→exception, exception→throwing-methods
    throws_out: HashMap<NodeId, Vec<NodeId>>,
    throws_in:  HashMap<NodeId, Vec<NodeId>>,

    // Awaits: async-fn→awaited-fn
    awaits_out: HashMap<NodeId, Vec<NodeId>>,

    // Captures: closure→captured-var
    captures_out: HashMap<NodeId, Vec<NodeId>>,

    // Decorates: annotated-node→annotation, annotation→decorated-nodes
    decorates_out: HashMap<NodeId, Vec<NodeId>>,
    decorates_in:  HashMap<NodeId, Vec<NodeId>>,

    // Nodes grouped by kind (debug string key)
    nodes_by_kind: HashMap<String, Vec<NodeId>>,
}

impl Indexes {
    fn build(graph: &DependencyGraph) -> Self {
        let mut idx = Self {
            contains_children: HashMap::new(),
            contains_parent:   HashMap::new(),
            calls_out:         HashMap::new(),
            calls_in:          HashMap::new(),
            implements_out:    HashMap::new(),
            implements_in:     HashMap::new(),
            extends_out:       HashMap::new(),
            extends_in:        HashMap::new(),
            overrides_out:     HashMap::new(),
            overrides_in:      HashMap::new(),
            reads_out:         HashMap::new(),
            reads_in:          HashMap::new(),
            writes_out:        HashMap::new(),
            writes_in:         HashMap::new(),
            imports_out:       HashMap::new(),
            imports_in:        HashMap::new(),
            instantiates_out:  HashMap::new(),
            instantiates_in:   HashMap::new(),
            has_type_out:      HashMap::new(),
            has_type_in:       HashMap::new(),
            returns_out:       HashMap::new(),
            returns_in:        HashMap::new(),
            throws_out:        HashMap::new(),
            throws_in:         HashMap::new(),
            awaits_out:        HashMap::new(),
            captures_out:      HashMap::new(),
            decorates_out:     HashMap::new(),
            decorates_in:      HashMap::new(),
            nodes_by_kind:     HashMap::new(),
        };

        for node in graph.nodes.values() {
            idx.nodes_by_kind
                .entry(format!("{:?}", node.kind))
                .or_default()
                .push(node.id);
        }

        for edge in &graph.edges {
            let from = edge.from;
            let to = match edge.to {
                EdgeTarget::Resolved(id) => id,
                _ => continue, // skip unresolved / external
            };

            // Helper closures for bidirectional and one-way indexing.
            let bi = |out: &mut HashMap<NodeId, Vec<NodeId>>,
                      inp: &mut HashMap<NodeId, Vec<NodeId>>| {
                out.entry(from).or_default().push(to);
                inp.entry(to).or_default().push(from);
            };

            match edge.kind {
                EdgeKind::Contains => {
                    idx.contains_children.entry(from).or_default().push(to);
                    idx.contains_parent.insert(to, from);
                }
                EdgeKind::Calls => {
                    bi(&mut idx.calls_out, &mut idx.calls_in);
                }
                EdgeKind::Implements => {
                    bi(&mut idx.implements_out, &mut idx.implements_in);
                }
                EdgeKind::Extends => {
                    bi(&mut idx.extends_out, &mut idx.extends_in);
                }
                EdgeKind::Overrides => {
                    bi(&mut idx.overrides_out, &mut idx.overrides_in);
                }
                EdgeKind::Reads => {
                    bi(&mut idx.reads_out, &mut idx.reads_in);
                }
                EdgeKind::Writes => {
                    bi(&mut idx.writes_out, &mut idx.writes_in);
                }
                EdgeKind::Imports => {
                    bi(&mut idx.imports_out, &mut idx.imports_in);
                }
                EdgeKind::Instantiates => {
                    bi(&mut idx.instantiates_out, &mut idx.instantiates_in);
                }
                EdgeKind::HasType => {
                    bi(&mut idx.has_type_out, &mut idx.has_type_in);
                }
                EdgeKind::Returns => {
                    bi(&mut idx.returns_out, &mut idx.returns_in);
                }
                EdgeKind::Throws => {
                    bi(&mut idx.throws_out, &mut idx.throws_in);
                }
                EdgeKind::Decorates => {
                    bi(&mut idx.decorates_out, &mut idx.decorates_in);
                }
                EdgeKind::Awaits => {
                    idx.awaits_out.entry(from).or_default().push(to);
                }
                EdgeKind::Captures => {
                    idx.captures_out.entry(from).or_default().push(to);
                }
                // Reexports, HasParameter, DependsOn, References: no dedicated index
                _ => {}
            }
        }

        idx
    }
}

// ─── GraphExplorer ────────────────────────────────────────────────────────────

pub struct GraphExplorer<'g> {
    graph: &'g DependencyGraph,
    idx:   Indexes,
}

impl<'g> GraphExplorer<'g> {
    /// Build the explorer and all indexes in one pass over the graph's edges.
    pub fn new(graph: &'g DependencyGraph) -> Self {
        Self { graph, idx: Indexes::build(graph) }
    }

    // ── Raw node access ───────────────────────────────────────────────────────

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.graph.get_node(id)
    }

    pub fn classes(&self)    -> Vec<NodeId> { self.nodes_of_kind(NodeKind::Class) }
    pub fn interfaces(&self) -> Vec<NodeId> { self.nodes_of_kind(NodeKind::Interface) }
    pub fn traits(&self)     -> Vec<NodeId> { self.nodes_of_kind(NodeKind::Trait) }
    pub fn enums(&self)      -> Vec<NodeId> { self.nodes_of_kind(NodeKind::Enum) }
    pub fn functions(&self)  -> Vec<NodeId> { self.nodes_of_kind(NodeKind::Function) }

    /// All node IDs of the given kind.
    pub fn nodes_of_kind(&self, kind: NodeKind) -> Vec<NodeId> {
        self.idx.nodes_by_kind
            .get(&format!("{:?}", kind))
            .cloned()
            .unwrap_or_default()
    }

    // ── Call graph ────────────────────────────────────────────────────────────

    /// All methods/functions transitively called by `root` (BFS over `Calls` edges).
    /// `depth = None` for unlimited traversal.
    pub fn downstream_calls(&self, root: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        self.bfs(&self.idx.calls_out, root, depth)
    }

    /// All methods/functions that transitively call `root` (reverse `Calls` edges).
    pub fn upstream_callers(&self, root: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        self.bfs(&self.idx.calls_in, root, depth)
    }

    /// Immediate callees of `id` (one hop).
    pub fn direct_callees(&self, id: NodeId) -> Vec<NodeId> {
        self.idx.calls_out.get(&id).cloned().unwrap_or_default()
    }

    /// Immediate callers of `id` (one hop).
    pub fn direct_callers(&self, id: NodeId) -> Vec<NodeId> {
        self.idx.calls_in.get(&id).cloned().unwrap_or_default()
    }

    /// Shortest call-chain from `from` to `to` (inclusive), or `None` if unreachable.
    pub fn call_path(&self, from: NodeId, to: NodeId) -> Option<Vec<NodeId>> {
        self.bfs_path(&self.idx.calls_out, from, to)
    }

    // ── Type hierarchy ────────────────────────────────────────────────────────

    /// All types that directly implement `interface_id`.
    pub fn implementors(&self, interface_id: NodeId) -> Vec<NodeId> {
        self.idx.implements_in.get(&interface_id).cloned().unwrap_or_default()
    }

    /// All subclasses of `class_id` (transitive, follows `Extends` edges downward).
    pub fn all_subclasses(&self, class_id: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        self.bfs(&self.idx.extends_in, class_id, depth)
    }

    /// Full superclass chain from `class_id` upward to the root class.
    pub fn superclass_chain(&self, class_id: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut current = class_id;
        let mut visited = HashSet::new();
        loop {
            match self.idx.extends_out.get(&current).and_then(|v| v.first()) {
                Some(&parent) if visited.insert(parent) => {
                    chain.push(parent);
                    current = parent;
                }
                _ => break,
            }
        }
        chain
    }

    /// Interfaces that extend `interface_id` (transitive).
    pub fn interface_hierarchy(&self, interface_id: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        self.bfs(&self.idx.extends_in, interface_id, depth)
    }

    /// All methods that have an `Overrides` edge targeting `method_id`.
    pub fn overriders(&self, method_id: NodeId) -> Vec<NodeId> {
        self.idx.overrides_in.get(&method_id).cloned().unwrap_or_default()
    }

    /// The method that `method_id` overrides (upward, one hop), if any.
    pub fn what_overrides(&self, method_id: NodeId) -> Option<NodeId> {
        self.idx.overrides_out.get(&method_id).and_then(|v| v.first()).copied()
    }

    /// Interface methods that `class_id` inherits but does not concretely implement
    /// (abstract-gap detection).
    pub fn unimplemented_interface_methods(&self, class_id: NodeId) -> Vec<NodeId> {
        let concrete: HashSet<String> = self.methods_of(class_id)
            .iter()
            .filter_map(|&id| self.graph.get_node(id).map(|n| n.name.clone()))
            .collect();

        self.idx.implements_out
            .get(&class_id)
            .into_iter()
            .flatten()
            .flat_map(|&iface_id| self.methods_of(iface_id))
            .filter(|&mid| {
                self.graph.get_node(mid)
                    .map_or(false, |m| m.is_abstract && !concrete.contains(&m.name))
            })
            .collect()
    }

    // ── Field / variable access ───────────────────────────────────────────────

    /// All methods that read `field_id`.
    pub fn readers_of(&self, field_id: NodeId) -> Vec<NodeId> {
        self.idx.reads_in.get(&field_id).cloned().unwrap_or_default()
    }

    /// All methods that write `field_id`.
    pub fn writers_of(&self, field_id: NodeId) -> Vec<NodeId> {
        self.idx.writes_in.get(&field_id).cloned().unwrap_or_default()
    }

    /// All fields/variables read inside `method_id`.
    pub fn fields_read_by(&self, method_id: NodeId) -> Vec<NodeId> {
        self.idx.reads_out.get(&method_id).cloned().unwrap_or_default()
    }

    /// All fields/variables written inside `method_id`.
    pub fn fields_written_by(&self, method_id: NodeId) -> Vec<NodeId> {
        self.idx.writes_out.get(&method_id).cloned().unwrap_or_default()
    }

    /// Fields of `type_id` with neither `Reads` nor `Writes` edges (potentially unused).
    pub fn unused_fields(&self, type_id: NodeId) -> Vec<NodeId> {
        self.fields_of(type_id)
            .into_iter()
            .filter(|&fid| {
                self.idx.reads_in.get(&fid).map_or(true, |v| v.is_empty())
                    && self.idx.writes_in.get(&fid).map_or(true, |v| v.is_empty())
            })
            .collect()
    }

    // ── Module / file structure ───────────────────────────────────────────────

    /// All nodes declared in `file`.
    pub fn nodes_in_file(&self, file: &str) -> Vec<NodeId> {
        self.graph.by_file.get(file).cloned().unwrap_or_default()
    }

    /// All nodes whose `qualified_name` starts with `package_prefix`.
    pub fn nodes_in_package(&self, package_prefix: &str) -> Vec<NodeId> {
        self.graph.nodes.values()
            .filter(|n| n.qualified_name.starts_with(package_prefix))
            .map(|n| n.id)
            .collect()
    }

    /// Files/packages that `node_id` directly imports (resolved only).
    pub fn direct_imports(&self, node_id: NodeId) -> Vec<NodeId> {
        self.idx.imports_out.get(&node_id).cloned().unwrap_or_default()
    }

    /// Nodes that directly import `node_id`.
    pub fn direct_importers(&self, node_id: NodeId) -> Vec<NodeId> {
        self.idx.imports_in.get(&node_id).cloned().unwrap_or_default()
    }

    /// Transitive import closure of `node_id`.
    pub fn import_closure(&self, node_id: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        self.bfs(&self.idx.imports_out, node_id, depth)
    }

    // ── Coupling & metrics ────────────────────────────────────────────────────

    /// Number of distinct nodes that depend on `type_id` (fan-in).
    pub fn afferent_coupling(&self, type_id: NodeId) -> usize {
        let mut dependents: HashSet<NodeId> = HashSet::new();
        for map in [
            &self.idx.calls_in,
            &self.idx.instantiates_in,
            &self.idx.implements_in,
            &self.idx.extends_in,
            &self.idx.has_type_in,
            &self.idx.returns_in,
        ] {
            dependents.extend(map.get(&type_id).into_iter().flatten());
        }
        dependents.len()
    }

    /// Number of distinct nodes that `type_id` depends on (fan-out).
    pub fn efferent_coupling(&self, type_id: NodeId) -> usize {
        let mut dependencies: HashSet<NodeId> = HashSet::new();
        for map in [
            &self.idx.calls_out,
            &self.idx.instantiates_out,
            &self.idx.implements_out,
            &self.idx.extends_out,
            &self.idx.has_type_out,
            &self.idx.returns_out,
        ] {
            dependencies.extend(map.get(&type_id).into_iter().flatten());
        }
        dependencies.len()
    }

    /// Robert Martin's instability: `efferent / (afferent + efferent)`.
    /// Returns `f64::NAN` when both are zero (isolated node).
    pub fn instability(&self, type_id: NodeId) -> f64 {
        let ce = self.efferent_coupling(type_id) as f64;
        let ca = self.afferent_coupling(type_id) as f64;
        if ca + ce == 0.0 { f64::NAN } else { ce / (ca + ce) }
    }

    /// All edges crossing between `type_a` and `type_b` in either direction.
    pub fn coupling_between(&self, type_a: NodeId, type_b: NodeId) -> Vec<&Edge> {
        self.graph.edges.iter()
            .filter(|e| {
                (e.from == type_a && e.to == EdgeTarget::Resolved(type_b))
                    || (e.from == type_b && e.to == EdgeTarget::Resolved(type_a))
            })
            .collect()
    }

    /// Top-`n` nodes by incoming-edge count (most depended-upon).
    pub fn hotspots(&self, n: usize) -> Vec<(NodeId, usize)> {
        let mut counts: Vec<(NodeId, usize)> = self.graph.nodes.keys()
            .map(|&id| (id, self.graph.edges_to.get(&id).map_or(0, |v| v.len())))
            .collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(n);
        counts
    }

    // ── Instantiation ─────────────────────────────────────────────────────────

    /// All methods/locations that instantiate `class_id`.
    pub fn instantiators(&self, class_id: NodeId) -> Vec<NodeId> {
        self.idx.instantiates_in.get(&class_id).cloned().unwrap_or_default()
    }

    /// All classes instantiated within `method_id`.
    pub fn types_instantiated_by(&self, method_id: NodeId) -> Vec<NodeId> {
        self.idx.instantiates_out.get(&method_id).cloned().unwrap_or_default()
    }

    // ── Annotations / attributes ──────────────────────────────────────────────

    /// All nodes that carry `attr` in their `attributes` vec (e.g. `"@Override"`).
    pub fn nodes_with_attribute(&self, attr: &str) -> Vec<NodeId> {
        self.graph.nodes.values()
            .filter(|n| n.attributes.iter().any(|a| a == attr))
            .map(|n| n.id)
            .collect()
    }

    /// All methods that declare a `Throws` edge to `exception_name`
    /// (matched against node name, qualified name, or unresolved/external string).
    pub fn methods_throwing(&self, exception_name: &str) -> Vec<NodeId> {
        self.graph.edges.iter()
            .filter(|e| matches!(e.kind, EdgeKind::Throws))
            .filter(|e| match &e.to {
                EdgeTarget::Resolved(tid) => self.graph.get_node(*tid)
                    .map_or(false, |n| {
                        n.name == exception_name || n.qualified_name == exception_name
                    }),
                EdgeTarget::External(s) | EdgeTarget::Unresolved(s) => {
                    s == exception_name
                        || s.ends_with(&format!(".{exception_name}"))
                        || s.ends_with(&format!("::{exception_name}"))
                }
            })
            .map(|e| e.from)
            .collect()
    }

    // ── Async ─────────────────────────────────────────────────────────────────

    /// Transitive async call tree following `Awaits` edges.
    pub fn async_call_chain(&self, method_id: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        self.bfs(&self.idx.awaits_out, method_id, depth)
    }

    // ── Structural summaries ──────────────────────────────────────────────────

    /// All `Method` nodes directly contained by `type_id`.
    pub fn methods_of(&self, type_id: NodeId) -> Vec<NodeId> {
        self.children_of_kind(type_id, |k| matches!(k, NodeKind::Method))
    }

    /// All `Field`, `StaticField`, and `Constant` nodes contained by `type_id`.
    pub fn fields_of(&self, type_id: NodeId) -> Vec<NodeId> {
        self.children_of_kind(type_id, |k| {
            matches!(k, NodeKind::Field | NodeKind::StaticField | NodeKind::Constant)
        })
    }

    /// All `Parameter` nodes of `fn_id`.
    pub fn parameters_of(&self, fn_id: NodeId) -> Vec<NodeId> {
        self.children_of_kind(fn_id, |k| matches!(k, NodeKind::Parameter))
    }

    /// Parent of `id` in the `Contains` hierarchy.
    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.idx.contains_parent.get(&id).copied()
    }

    /// All `ExternalPackage` nodes.
    pub fn external_dependencies(&self) -> Vec<NodeId> {
        self.nodes_of_kind(NodeKind::ExternalPackage)
    }

    /// Node and edge counts grouped by kind.
    pub fn summary(&self) -> Summary {
        let mut node_counts: HashMap<String, usize> = HashMap::new();
        for n in self.graph.nodes.values() {
            *node_counts.entry(format!("{:?}", n.kind)).or_default() += 1;
        }
        let mut edge_counts: HashMap<String, usize> = HashMap::new();
        for e in &self.graph.edges {
            *edge_counts.entry(format!("{:?}", e.kind)).or_default() += 1;
        }
        Summary {
            total_nodes: self.graph.node_count(),
            total_edges: self.graph.edge_count(),
            node_counts,
            edge_counts,
        }
    }

    // ── Dead code & reachability ──────────────────────────────────────────────

    /// All public `Method`/`Function` nodes with no internal callers
    /// (true API entry points).
    pub fn entry_points(&self) -> Vec<NodeId> {
        self.graph.nodes.values()
            .filter(|n| {
                matches!(n.kind, NodeKind::Method | NodeKind::Function)
                    && n.visibility == Visibility::Public
                    && self.idx.calls_in.get(&n.id).map_or(true, |v| v.is_empty())
            })
            .map(|n| n.id)
            .collect()
    }

    /// Transitive closure of nodes reachable from `roots` via `Calls` and
    /// `Instantiates` edges. `roots` themselves are included in the result.
    pub fn reachable_from(&self, roots: &[NodeId], depth: Option<usize>) -> HashSet<NodeId> {
        let max_depth = depth.unwrap_or(usize::MAX);
        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, usize)> =
            roots.iter().map(|&id| (id, 0)).collect();

        while let Some((id, d)) = queue.pop_front() {
            if !visited.insert(id) { continue; }
            if d >= max_depth { continue; }
            for &next in self.idx.calls_out.get(&id).into_iter().flatten()
                .chain(self.idx.instantiates_out.get(&id).into_iter().flatten())
            {
                if !visited.contains(&next) {
                    queue.push_back((next, d + 1));
                }
            }
        }
        visited
    }

    /// All nodes NOT reachable from `roots` (dead-code candidates).
    pub fn dead_code(&self, roots: &[NodeId]) -> Vec<NodeId> {
        let reachable = self.reachable_from(roots, None);
        self.graph.nodes.keys()
            .filter(|id| !reachable.contains(id))
            .copied()
            .collect()
    }

    /// Reverse closure: everything that may need to change if `id` changes.
    /// Follows `Calls`, `Instantiates`, `Reads`, `Writes`, `HasType`,
    /// `Returns`, `Extends`, `Implements` edges in reverse.
    pub fn change_impact(&self, id: NodeId, depth: Option<usize>) -> HashSet<NodeId> {
        // Build a temporary reverse-adjacency map for impact-relevant edge kinds.
        let mut reverse: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in &self.graph.edges {
            if matches!(
                edge.kind,
                EdgeKind::Calls
                    | EdgeKind::Instantiates
                    | EdgeKind::Reads
                    | EdgeKind::Writes
                    | EdgeKind::HasType
                    | EdgeKind::Returns
                    | EdgeKind::Extends
                    | EdgeKind::Implements
            ) {
                if let EdgeTarget::Resolved(to) = edge.to {
                    reverse.entry(to).or_default().push(edge.from);
                }
            }
        }
        let reachable = self.bfs(&reverse, id, depth);
        reachable.into_iter().collect()
    }

    // ── Type usages ───────────────────────────────────────────────────────────

    /// All nodes referencing `type_id` via `HasType`, `Returns`, `Instantiates`,
    /// or `References` edges.
    pub fn usages_of_type(&self, type_id: NodeId) -> Vec<NodeId> {
        let mut users: HashSet<NodeId> = HashSet::new();
        for map in [
            &self.idx.has_type_in,
            &self.idx.returns_in,
            &self.idx.instantiates_in,
        ] {
            users.extend(map.get(&type_id).into_iter().flatten());
        }
        // Also pick up any References edges (no dedicated index).
        for edge in &self.graph.edges {
            if matches!(edge.kind, EdgeKind::References)
                && edge.to == EdgeTarget::Resolved(type_id)
            {
                users.insert(edge.from);
            }
        }
        users.into_iter().collect()
    }

    /// All `Public` children of `type_id` (the external contract).
    pub fn public_api(&self, type_id: NodeId) -> Vec<NodeId> {
        self.idx.contains_children
            .get(&type_id)
            .into_iter()
            .flatten()
            .filter(|&&id| {
                self.graph.get_node(id)
                    .map_or(false, |n| n.visibility == Visibility::Public)
            })
            .copied()
            .collect()
    }

    // ── Package / architecture ────────────────────────────────────────────────

    /// Package qualified-name of `id`: walk up the `Contains` hierarchy to
    /// the nearest `Package` node; fall back to the two-segment qualified-name prefix.
    pub fn package_of(&self, id: NodeId) -> Option<String> {
        let mut current = id;
        loop {
            if let Some(node) = self.graph.get_node(current) {
                if node.kind == NodeKind::Package {
                    return Some(node.qualified_name.clone());
                }
            }
            current = *self.idx.contains_parent.get(&current)?;
        }
    }

    /// Collapse the full graph to a package→package dependency view.
    /// Only cross-package edges from `Calls`, `Instantiates`, `Implements`,
    /// `Extends`, `Imports`, and `References` are counted.
    pub fn package_dependency_graph(&self) -> Vec<PackageDependency> {
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for edge in &self.graph.edges {
            if !matches!(
                edge.kind,
                EdgeKind::Calls
                    | EdgeKind::Instantiates
                    | EdgeKind::Implements
                    | EdgeKind::Extends
                    | EdgeKind::Imports
                    | EdgeKind::References
            ) {
                continue;
            }
            let to = match edge.to {
                EdgeTarget::Resolved(id) => id,
                _ => continue,
            };
            let (Some(fp), Some(tp)) =
                (self.package_of(edge.from), self.package_of(to))
            else {
                continue;
            };
            if fp != tp {
                *counts.entry((fp, tp)).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .map(|((from, to), edge_count)| PackageDependency { from, to, edge_count })
            .collect()
    }

    /// Detect cycles specifically between packages (returns each cycle as
    /// an ordered Vec of package names).
    pub fn package_cycles(&self) -> Vec<Vec<String>> {
        let deps = self.package_dependency_graph();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for d in &deps {
            adj.entry(d.from.clone()).or_default().push(d.to.clone());
        }
        let nodes: Vec<String> = adj.keys().cloned().collect();
        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = Vec::new();
        let mut cycles: Vec<Vec<String>> = Vec::new();
        for node in nodes {
            if !visited.contains(&node) {
                Self::dfs_cycle_str(&node, &adj, &mut visited, &mut stack, &mut cycles);
            }
        }
        cycles
    }

    /// Detect cycles in the full node graph over `Extends` and `Imports` edges.
    pub fn cycles(&self) -> Vec<Vec<NodeId>> {
        let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for edge in &self.graph.edges {
            if matches!(edge.kind, EdgeKind::Extends | EdgeKind::Imports) {
                if let EdgeTarget::Resolved(to) = edge.to {
                    adj.entry(edge.from).or_default().push(to);
                }
            }
        }
        let nodes: Vec<NodeId> = adj.keys().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<NodeId> = Vec::new();
        let mut cycles: Vec<Vec<NodeId>> = Vec::new();
        for node in nodes {
            if !visited.contains(&node) {
                Self::dfs_cycle_id(node, &adj, &mut visited, &mut stack, &mut cycles);
            }
        }
        cycles
    }

    /// Detect edges that violate a layer ordering.
    ///
    /// `layers` is an ordered slice of slices of qualified-name prefixes, from
    /// highest (index 0) to lowest (last index). An edge from layer `i` to layer
    /// `j` where `i > j` (i.e. a *lower* layer calling a *higher* one) is a
    /// violation.
    pub fn layer_violations(&self, layers: &[&[&str]]) -> Vec<LayerViolation> {
        let layer_of = |id: NodeId| -> Option<usize> {
            let qn = self.graph.get_node(id)?.qualified_name.as_str();
            layers.iter().enumerate().find_map(|(i, prefixes)| {
                prefixes.iter().any(|p| qn.starts_with(*p)).then_some(i)
            })
        };

        self.graph.edges.iter()
            .filter(|e| matches!(
                e.kind,
                EdgeKind::Calls | EdgeKind::Instantiates | EdgeKind::Imports | EdgeKind::References
            ))
            .filter_map(|e| {
                let to = match e.to { EdgeTarget::Resolved(id) => id, _ => return None };
                let fl = layer_of(e.from)?;
                let tl = layer_of(to)?;
                (fl > tl).then_some(LayerViolation {
                    from_node: e.from,
                    to_node: to,
                    from_layer: fl,
                    to_layer: tl,
                })
            })
            .collect()
    }

    // ── Cohesion ──────────────────────────────────────────────────────────────

    /// Lack of Cohesion of Methods (Henderson-Sellers variant).
    ///
    /// Returns a value in `[0.0, 1.0]`: 0 = perfectly cohesive,
    /// 1 = completely non-cohesive. Returns `0.0` for types with fewer than 2 methods.
    pub fn lcom(&self, type_id: NodeId) -> f64 {
        let methods = self.methods_of(type_id);
        if methods.len() < 2 { return 0.0; }

        let field_sets: Vec<HashSet<NodeId>> = methods.iter()
            .map(|&mid| {
                let mut s: HashSet<NodeId> = self.idx.reads_out
                    .get(&mid).into_iter().flatten().copied().collect();
                s.extend(self.idx.writes_out.get(&mid).into_iter().flatten().copied());
                s
            })
            .collect();

        let n = methods.len();
        let total_pairs = n * (n - 1) / 2;
        if total_pairs == 0 { return 0.0; }

        let sharing = (0..n)
            .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
            .filter(|&(i, j)| !field_sets[i].is_disjoint(&field_sets[j]))
            .count();

        let non_sharing = total_pairs.saturating_sub(sharing);
        if non_sharing > sharing {
            (non_sharing - sharing) as f64 / total_pairs as f64
        } else {
            0.0
        }
    }

    // ── Exception / error flow ────────────────────────────────────────────────

    /// All methods that declare or propagate `exception_name` through the call chain
    /// (conservative: any caller of a throwing method is considered a propagator).
    pub fn exception_propagation(&self, exception_name: &str) -> Vec<NodeId> {
        let direct = self.methods_throwing(exception_name);
        let mut seen: HashSet<NodeId> = direct.iter().copied().collect();
        let mut frontier: VecDeque<NodeId> = direct.into_iter().collect();
        while let Some(id) = frontier.pop_front() {
            for &caller in self.idx.calls_in.get(&id).into_iter().flatten() {
                if seen.insert(caller) {
                    frontier.push_back(caller);
                }
            }
        }
        seen.into_iter().collect()
    }

    // ── Shared coupling ───────────────────────────────────────────────────────

    /// Types that both `a` and `b` depend on (intersection of their efferent sets).
    pub fn shared_dependencies(&self, a: NodeId, b: NodeId) -> Vec<NodeId> {
        let deps_of = |id: NodeId| -> HashSet<NodeId> {
            [
                &self.idx.calls_out,
                &self.idx.instantiates_out,
                &self.idx.has_type_out,
                &self.idx.returns_out,
            ]
            .iter()
            .flat_map(|m| m.get(&id).into_iter().flatten().copied())
            .collect()
        };
        let da = deps_of(a);
        let db = deps_of(b);
        da.intersection(&db).copied().collect()
    }

    /// Variables captured by `closure_id` (via `Captures` edges).
    pub fn closure_captures(&self, closure_id: NodeId) -> Vec<NodeId> {
        self.idx.captures_out.get(&closure_id).cloned().unwrap_or_default()
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// BFS from `root` over `adj`; returns all visited nodes except `root` itself.
    fn bfs(&self, adj: &HashMap<NodeId, Vec<NodeId>>, root: NodeId, depth: Option<usize>) -> Vec<NodeId> {
        let max_depth = depth.unwrap_or(usize::MAX);
        let mut visited: HashSet<NodeId> = HashSet::from([root]);
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(root, 0)]);
        let mut result = Vec::new();
        while let Some((id, d)) = queue.pop_front() {
            if d >= max_depth { continue; }
            for &next in adj.get(&id).into_iter().flatten() {
                if visited.insert(next) {
                    result.push(next);
                    queue.push_back((next, d + 1));
                }
            }
        }
        result
    }

    /// BFS shortest path between `from` and `to` (inclusive). Returns `None` if unreachable.
    fn bfs_path(
        &self,
        adj: &HashMap<NodeId, Vec<NodeId>>,
        from: NodeId,
        to: NodeId,
    ) -> Option<Vec<NodeId>> {
        if from == to { return Some(vec![from]); }
        let mut visited: HashSet<NodeId> = HashSet::from([from]);
        let mut queue: VecDeque<Vec<NodeId>> = VecDeque::from([vec![from]]);
        while let Some(path) = queue.pop_front() {
            let last = *path.last().unwrap();
            for &next in adj.get(&last).into_iter().flatten() {
                if next == to {
                    let mut p = path;
                    p.push(next);
                    return Some(p);
                }
                if visited.insert(next) {
                    let mut p = path.clone();
                    p.push(next);
                    queue.push_back(p);
                }
            }
        }
        None
    }

    fn children_of_kind<F>(&self, parent: NodeId, pred: F) -> Vec<NodeId>
    where
        F: Fn(&NodeKind) -> bool,
    {
        self.idx.contains_children
            .get(&parent)
            .into_iter()
            .flatten()
            .filter(|&&id| self.graph.get_node(id).map_or(false, |n| pred(&n.kind)))
            .copied()
            .collect()
    }

    fn dfs_cycle_str(
        node: &str,
        adj: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        stack: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(node.to_string());
        stack.push(node.to_string());
        for next in adj.get(node).into_iter().flatten() {
            if !visited.contains(next) {
                Self::dfs_cycle_str(next, adj, visited, stack, cycles);
            } else if let Some(pos) = stack.iter().position(|x| x == next) {
                cycles.push(stack[pos..].to_vec());
            }
        }
        stack.pop();
    }

    fn dfs_cycle_id(
        node: NodeId,
        adj: &HashMap<NodeId, Vec<NodeId>>,
        visited: &mut HashSet<NodeId>,
        stack: &mut Vec<NodeId>,
        cycles: &mut Vec<Vec<NodeId>>,
    ) {
        visited.insert(node);
        stack.push(node);
        for &next in adj.get(&node).into_iter().flatten() {
            if !visited.contains(&next) {
                Self::dfs_cycle_id(next, adj, visited, stack, cycles);
            } else if let Some(pos) = stack.iter().position(|&x| x == next) {
                cycles.push(stack[pos..].to_vec());
            }
        }
        stack.pop();
    }
}
