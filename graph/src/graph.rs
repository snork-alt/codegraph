use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ─── Identity ────────────────────────────────────────────────────────────────

pub type NodeId = u64;
pub type EdgeId = u64;

/// Source location inside a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl Span {
    pub fn new(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Self {
        Self { start_line, start_col, end_line, end_col }
    }
}

// ─── Language ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Java,
    Python,
    TypeScript,
    JavaScript,
    Rust,
    Go,
    CSharp,
    Cpp,
    Ruby,
    Swift,
    Kotlin,
    Unknown,
}

// ─── Visibility ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Protected,
    Private,
    Internal,       // C# internal, Rust pub(crate)
    PackagePrivate, // Java default
    Unspecified,
}

impl Default for Visibility {
    fn default() -> Self {
        Visibility::Unspecified
    }
}

// ─── Node ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Package,
    Class,
    Interface,
    Trait,
    Enum,
    Annotation,      // Java @interface, Python decorator class
    TypeAlias,       // Rust `type Foo = Bar`, TS `type X = Y`
    Function,        // standalone function
    Method,          // function scoped to a type
    Field,           // instance variable / property
    StaticField,     // class-level / static variable
    Constant,        // const / final — never mutated
    Variable,        // local variable inside a function/method
    Parameter,       // function/method parameter
    TypeParameter,   // generic type parameter `T`, `K`
    Closure,         // lambda / anonymous function
    GlobalVariable,  // module-level var outside any class
    Import,          // import / use / require declaration
    ExternalPackage, // dependency declared in a manifest (Cargo.toml, pom.xml, …)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,

    /// Simple unqualified name, e.g. `"myMethod"`.
    pub name: String,

    /// Fully-qualified name, e.g. `"com.example.Foo.myMethod"`.
    pub qualified_name: String,

    /// Source file path.
    pub file: String,

    pub span: Span,

    pub language: Language,

    pub visibility: Visibility,

    /// True for async functions/methods.
    pub is_async: bool,

    /// True for abstract methods / virtual methods (no body).
    pub is_abstract: bool,

    /// True for constructors (`__init__`, `constructor`, `new`).
    pub is_constructor: bool,

    /// True when this node is part of a test suite (test file, test class, or
    /// test method/function).  Set by static file-name heuristics and confirmed
    /// (or initially set) by LLM enrichment during the description pass.
    pub is_test: bool,

    /// Declared return type or field type as a raw string.
    pub type_annotation: Option<String>,

    /// Names of declared generic type parameters, e.g. `["T", "K"]`.
    pub generic_params: Vec<String>,

    /// Raw generic bound strings, e.g. `["T: Display + Clone"]`.
    pub generic_bounds: Vec<String>,

    /// Decorators / annotations applied to this node, e.g. `["@Override"]`.
    pub attributes: Vec<String>,

    /// Arbitrary language-specific extras (e.g. `"native" => "true"`).
    pub metadata: HashMap<String, String>,

    /// Optional human-readable description (doc-comment, summary, etc.).
    pub description: Option<String>,

    /// SHA-256 hex digest of the file's source content.
    /// Populated only for `NodeKind::File` nodes, `None` for all others.
    pub hash: Option<String>,
}

impl Node {
    pub fn new(
        id: NodeId,
        kind: NodeKind,
        name: impl Into<String>,
        qualified_name: impl Into<String>,
        file: impl Into<String>,
        span: Span,
        language: Language,
    ) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            qualified_name: qualified_name.into(),
            file: file.into(),
            span,
            language,
            visibility: Visibility::default(),
            is_async: false,
            is_abstract: false,
            is_constructor: false,
            is_test: false,
            type_annotation: None,
            generic_params: Vec::new(),
            generic_bounds: Vec::new(),
            attributes: Vec::new(),
            metadata: HashMap::new(),
            description: None,
            hash: None,
        }
    }
}

// ─── Edge ─────────────────────────────────────────────────────────────────────

/// The resolved/unresolved target of an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeTarget {
    /// Successfully linked to a node in this graph.
    Resolved(NodeId),
    /// Refers to something outside the parsed codebase (stdlib, 3rd-party).
    External(String),
    /// Referenced by name but not yet resolved (pre-resolution state).
    Unresolved(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Structural parent → child containment.
    Contains,
    /// File / package imports another symbol or module.
    Imports,
    /// Re-exports a symbol (`pub use`, `export { X } from`).
    Reexports,
    /// Class extends / inherits from another class.
    Extends,
    /// Class implements an interface / satisfies a trait.
    Implements,
    /// Method in a subclass overrides a parent method.
    Overrides,
    /// Function / method declares a parameter.
    HasParameter,
    /// Function / method has a declared return type.
    Returns,
    /// Node has a declared type annotation (field, variable, parameter).
    HasType,
    /// Function / method calls another function / method.
    Calls,
    /// Code instantiates a class (`new Foo(…)`, struct literal).
    Instantiates,
    /// Function / method reads a variable, field, or constant.
    Reads,
    /// Function / method writes / assigns a variable or field.
    Writes,
    /// Function / method declares it can throw / raise an exception type.
    Throws,
    /// Async function / method awaits another async function.
    Awaits,
    /// Closure captures a variable from an outer scope.
    Captures,
    /// Annotation / decorator is applied to a node.
    Decorates,
    /// File / package depends on an external package (manifest-level).
    DependsOn,
    /// General type reference (fallback when a more specific kind doesn't apply).
    References,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub kind: EdgeKind,
    pub from: NodeId,
    pub to: EdgeTarget,

    /// Where in source this relationship appears.
    pub span: Span,

    /// For `Calls` edges: the number of arguments passed (helps overload resolution).
    pub call_arity: Option<u32>,

    /// For `Decorates` edges: raw argument string of the annotation.
    pub annotation_args: Option<String>,
}

impl Edge {
    pub fn new(id: EdgeId, kind: EdgeKind, from: NodeId, to: EdgeTarget, span: Span) -> Self {
        Self {
            id,
            kind,
            from,
            to,
            span,
            call_arity: None,
            annotation_args: None,
        }
    }
}

// ─── Graph ────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DependencyGraph {
    pub nodes: HashMap<NodeId, Node>,
    pub edges: Vec<Edge>,

    // Indices — rebuilt after extraction / resolution passes.
    pub edges_from: HashMap<NodeId, Vec<EdgeId>>,
    pub edges_to: HashMap<NodeId, Vec<EdgeId>>,

    /// Maps `qualified_name` → `NodeId` for the resolution pass.
    pub by_qualified: HashMap<String, NodeId>,

    /// Maps file path → list of top-level `NodeId`s declared in that file.
    pub by_file: HashMap<String, Vec<NodeId>>,

    next_node_id: NodeId,
    next_edge_id: EdgeId,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Node helpers ──────────────────────────────────────────────────────────

    pub fn add_node(&mut self, mut node: Node) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        node.id = id;

        self.by_qualified.insert(node.qualified_name.clone(), id);
        self.by_file.entry(node.file.clone()).or_default().push(id);
        self.nodes.insert(id, node);
        id
    }

    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    pub fn find_by_qualified(&self, qname: &str) -> Option<NodeId> {
        self.by_qualified.get(qname).copied()
    }

    // ── Edge helpers ──────────────────────────────────────────────────────────

    pub fn add_edge(&mut self, mut edge: Edge) -> EdgeId {
        let id = self.next_edge_id;
        self.next_edge_id += 1;
        edge.id = id;

        self.edges_from.entry(edge.from).or_default().push(id);
        if let EdgeTarget::Resolved(to) = edge.to {
            self.edges_to.entry(to).or_default().push(id);
        }
        self.edges.push(edge);
        id
    }

    pub fn add_edge_simple(
        &mut self,
        kind: EdgeKind,
        from: NodeId,
        to: EdgeTarget,
        span: Span,
    ) -> EdgeId {
        let id = self.next_edge_id;
        let edge = Edge::new(id, kind, from, to, span);
        self.add_edge(edge)
    }

    // ── Resolution pass ───────────────────────────────────────────────────────

    /// Resolve all `Unresolved` edge targets against the graph's qualified-name index.
    /// Unresolvable names are downgraded to `External`.
    pub fn resolve(&mut self) {
        for edge in &mut self.edges {
            if let EdgeTarget::Unresolved(ref name) = edge.to.clone() {
                edge.to = match self.by_qualified.get(name.as_str()) {
                    Some(&node_id) => EdgeTarget::Resolved(node_id),
                    None => EdgeTarget::External(name.clone()),
                };
                // Rebuild edges_to index for newly resolved edges.
                if let EdgeTarget::Resolved(to) = edge.to {
                    self.edges_to.entry(to).or_default().push(edge.id);
                }
            }
        }
    }

    // ── File removal ──────────────────────────────────────────────────────────

    /// Remove all nodes (and their edges) that belong to `path`.
    ///
    /// Cross-file edges whose `to` target was `Resolved` to one of the removed
    /// nodes are converted back to `Unresolved(qualified_name)` so that the
    /// next `resolve()` call can re-link them if a replacement node exists.
    pub fn remove_file(&mut self, path: &str) {
        use std::collections::{HashMap as HM, HashSet};

        // 1. Collect the IDs of every node declared in this file.
        let removed_ids: HashSet<NodeId> = self
            .by_file
            .get(path)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default();

        if removed_ids.is_empty() {
            return;
        }

        // 2. Before erasing the nodes, record each id → qualified_name so we
        //    can rewrite Resolved edges that point at them.
        let id_to_qname: HM<NodeId, String> = removed_ids
            .iter()
            .filter_map(|&id| {
                self.nodes.get(&id).map(|n| (id, n.qualified_name.clone()))
            })
            .collect();

        // 3. Remove the nodes from every index.
        for &id in &removed_ids {
            if let Some(node) = self.nodes.remove(&id) {
                self.by_qualified.remove(&node.qualified_name);
            }
        }
        self.by_file.remove(path);

        // 4. Rebuild the edge list:
        //    • Drop edges whose `from` node was removed.
        //    • Rewrite `Resolved(id)` targets for removed ids → `Unresolved(qname)`.
        let mut kept: Vec<Edge> = Vec::with_capacity(self.edges.len());
        for mut edge in std::mem::take(&mut self.edges) {
            if removed_ids.contains(&edge.from) {
                continue; // drop edge
            }
            if let EdgeTarget::Resolved(to_id) = edge.to {
                if let Some(qname) = id_to_qname.get(&to_id) {
                    edge.to = EdgeTarget::Unresolved(qname.clone());
                }
            }
            kept.push(edge);
        }
        self.edges = kept;

        // 5. Rebuild the edges_from / edges_to indices from scratch.
        self.edges_from.clear();
        self.edges_to.clear();
        for edge in &self.edges {
            self.edges_from.entry(edge.from).or_default().push(edge.id);
            if let EdgeTarget::Resolved(to) = edge.to {
                self.edges_to.entry(to).or_default().push(edge.id);
            }
        }
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    /// Peek at the NodeId that will be assigned on the next `add_node` call,
    /// without actually allocating it. Useful when you need to create edges
    /// that reference a node before it is inserted.
    pub fn next_node_id_peek(&self) -> NodeId {
        self.next_node_id
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}
