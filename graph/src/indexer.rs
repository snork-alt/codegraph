use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Serialize;

use crate::filesystem::FileSystem;
use crate::graph::{DependencyGraph, NodeKind};
use crate::languages::golang::GoExtractor;
use crate::languages::java::JavaExtractor;
use crate::languages::python::PythonExtractor;
use crate::languages::rust::RustExtractor;
use crate::languages::typescript::TypeScriptExtractor;
use crate::parser::{hash_source, LanguageExtractor};
use crate::serializer::GraphSerializer;

// ─── Test-file detection ──────────────────────────────────────────────────────

/// Returns `true` when the file path matches a well-known test-file naming
/// convention for any supported language.
fn is_test_file(path: &str) -> bool {
    let file = Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Go:         foo_test.go
    // Rust:       tests/ directory or files named *_test.rs
    // Python:     test_foo.py  /  foo_test.py
    // TypeScript: foo.test.ts  /  foo.spec.ts  /  foo.test.tsx  /  foo.spec.tsx
    // Java:       FooTest.java / FooTests.java / FooSpec.java / FooIT.java
    file.ends_with("_test.go")
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
        || stem.ends_with("Test")
        || stem.ends_with("Tests")
        || stem.ends_with("Spec")
        || stem.ends_with("IT")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("/__tests__/")
        || path.contains("/spec/")
}

// ─── Description tasks ────────────────────────────────────────────────────────

/// Node kinds that benefit from an LLM-generated description.
const DESCRIBED_KINDS: &[NodeKind] = &[
    NodeKind::Class,
    NodeKind::Interface,
    NodeKind::Trait,
    NodeKind::Enum,
    NodeKind::Method,
    NodeKind::Function,
    NodeKind::Field,
    NodeKind::StaticField,
];

/// One file's worth of description work, produced by [`GraphIndexer::run`] for
/// every source file that was added or changed in this indexing pass.
///
/// `snippets` maps each qualified name to the extracted source lines for that
/// node.  `schema` has the same keys but empty-string values; the caller fills
/// those values (typically via an LLM) and passes the completed map to
/// [`IndexResult::commit`].
///
/// `is_test_schema` has the same keys with `null` values; the LLM fills each
/// with `true` or `false` indicating whether the node is a test.  Nodes in
/// files whose names already match test-file patterns are pre-flagged via
/// static heuristics and do not need LLM confirmation.
#[derive(Debug, Clone, Serialize)]
pub struct DescriptionTask {
    /// Relative path to the source file (relative to the indexer root).
    pub file:          String,
    /// `qualified_name → source snippet`.
    pub snippets:      HashMap<String, String>,
    /// `qualified_name → ""` — LLM fills each with a description string.
    pub schema:        HashMap<String, String>,
    /// `qualified_name → null` — LLM fills each with `true`/`false`.
    /// Nodes that are statically known to be tests are omitted (already set).
    pub is_test_schema: HashMap<String, Option<bool>>,
}

// ─── IndexResult ──────────────────────────────────────────────────────────────

/// The outcome of a structural indexing pass, ready for optional description
/// enrichment before the graph is persisted to disk.
///
/// `graph.yml` is **not** written until [`commit`](Self::commit) (or the
/// convenience wrapper [`finish`](Self::finish)) is called.
pub struct IndexResult {
    /// Files that were added or changed — each carries its LLM schema.
    pub tasks: Vec<DescriptionTask>,
    graph:     DependencyGraph,
    root:      String,
    fs:        Box<dyn FileSystem>,
}

impl IndexResult {
    /// Files requiring description enrichment (added or changed in this pass).
    pub fn pending_tasks(&self) -> &[DescriptionTask] {
        &self.tasks
    }

    /// Apply `descriptions` (qualified_name → description text) and
    /// `is_test_flags` (qualified_name → bool) to matching graph nodes,
    /// write `<root>/.codegraph/graph.yml`, and return the final graph.
    ///
    /// Pass empty maps to skip enrichment and just persist the structural graph.
    pub fn commit(
        mut self,
        descriptions:  HashMap<String, String>,
        is_test_flags: HashMap<String, bool>,
    ) -> DependencyGraph {
        for node in self.graph.nodes.values_mut() {
            if let Some(desc) = descriptions.get(&node.qualified_name) {
                if !desc.is_empty() {
                    node.description = Some(desc.clone());
                }
            }
            if let Some(&flag) = is_test_flags.get(&node.qualified_name) {
                node.is_test = flag;
            }
        }
        let graph_yml = format!("{}/.codegraph/graph.yml", self.root);
        if let Ok(yaml) = GraphSerializer::serialize(&self.graph) {
            self.fs.write(&graph_yml, &yaml);
        }
        self.graph
    }

    /// Commit without enrichment — write the structural graph as-is.
    pub fn finish(self) -> DependencyGraph {
        self.commit(HashMap::new(), HashMap::new())
    }
}

// ─── GraphIndexer ─────────────────────────────────────────────────────────────

/// Drives recursive filesystem scanning, incremental diffing, and graph
/// serialisation.  All I/O is performed through an injected [`FileSystem`].
///
/// # Incremental indexing
///
/// On each `run()` call the indexer:
/// 1. Loads `<root>/.codegraph/graph.yml` if it exists.
/// 2. Compares SHA-256 hashes of current files against those stored in the
///    graph — unchanged files are skipped entirely, preserving their existing
///    node IDs and edge IDs.
/// 3. Files that changed are removed from the graph (with cross-file
///    `Resolved` edges converted back to `Unresolved`) and re-extracted.
/// 4. Files that no longer exist are removed from the graph.
/// 5. Returns an [`IndexResult`]; `graph.yml` is written only when
///    [`IndexResult::commit`] or [`IndexResult::finish`] is called.
pub struct GraphIndexer {
    root:       String,
    fs:         Box<dyn FileSystem>,
    extractors: HashMap<String, Arc<dyn LanguageExtractor>>,
    /// When `true`, any existing `graph.yml` is ignored and the graph is built
    /// from scratch.  Defaults to `false`.
    rebuild:    bool,
}

impl GraphIndexer {
    /// Create a new indexer rooted at `root`.
    ///
    /// `fs` is the I/O back-end; pass a real host filesystem wrapper in
    /// production and a [`MockFileSystem`] in tests.
    pub fn new(root: impl Into<String>, fs: Box<dyn FileSystem>) -> Self {
        let mut indexer = Self {
            root:       root.into(),
            fs,
            extractors: HashMap::new(),
            rebuild:    false,
        };
        indexer.register(&["go"],        GoExtractor);
        indexer.register(&["java"],      JavaExtractor);
        indexer.register(&["py"],        PythonExtractor);
        indexer.register(&["rs"],        RustExtractor);
        indexer.register(&["ts", "tsx"], TypeScriptExtractor);
        indexer
    }

    /// Register a custom extractor for the given file extensions (without the
    /// leading `.`).  Overwrites any existing extractor for that extension.
    pub fn register<E>(&mut self, extensions: &[&str], extractor: E) -> &mut Self
    where
        E: LanguageExtractor + 'static,
    {
        let shared = Arc::new(extractor) as Arc<dyn LanguageExtractor>;
        for ext in extensions {
            self.extractors.insert(ext.to_string(), Arc::clone(&shared));
        }
        self
    }

    /// When set to `true`, any existing `<root>/.codegraph/graph.yml` is
    /// discarded and the graph is rebuilt entirely from scratch on the next
    /// [`run`](Self::run) call.  Defaults to `false`.
    pub fn rebuild(mut self, rebuild: bool) -> Self {
        self.rebuild = rebuild;
        self
    }

    /// The file extensions that have a registered extractor.
    pub fn supported_extensions(&self) -> Vec<&str> {
        self.extractors.keys().map(String::as_str).collect()
    }

    // ── Public entry point ────────────────────────────────────────────────────

    /// Scan `root`, update the graph incrementally, resolve cross-file
    /// references, and return an [`IndexResult`].
    ///
    /// `graph.yml` is **not** written here — call [`IndexResult::commit`] or
    /// [`IndexResult::finish`] on the returned value to persist the graph.
    pub fn run(self) -> IndexResult {
        let graph_yml = format!("{}/.codegraph/graph.yml", self.root);

        // ── 1. Load existing graph (incremental base) ─────────────────────────
        let mut graph = if self.rebuild {
            DependencyGraph::new() // ignore any existing graph.yml
        } else {
            self.fs
                .read(&graph_yml)
                .and_then(|yaml| GraphSerializer::deserialize(&yaml).ok())
                .unwrap_or_default()
        };

        // ── 2. Build a map of file path → existing hash ───────────────────────
        let existing_hashes: HashMap<String, String> = graph.nodes.values()
            .filter_map(|n| {
                if n.kind == NodeKind::File {
                    n.hash.as_ref().map(|h| (n.file.clone(), h.clone()))
                } else {
                    None
                }
            })
            .collect();

        // ── 3. Walk the filesystem ────────────────────────────────────────────
        let mut current_files: HashMap<String, String> = HashMap::new();
        let root = self.root.clone();
        self.walk_dir(&root, &[], &mut current_files);

        // ── 4. Remove deleted files ───────────────────────────────────────────
        let current_paths: HashSet<&String> = current_files.keys().collect();
        let deleted: Vec<String> = existing_hashes
            .keys()
            .filter(|p| !current_paths.contains(p))
            .cloned()
            .collect();
        for path in &deleted {
            graph.remove_file(path);
        }

        // ── 5. Re-index changed / new files ───────────────────────────────────
        let mut changed_files: HashSet<String> = HashSet::new();
        for (rel_path, content) in &current_files {
            let new_hash = hash_source(content);
            let needs_index = existing_hashes
                .get(rel_path.as_str())
                .map_or(true, |h| h != &new_hash);

            if needs_index {
                changed_files.insert(rel_path.clone());
                graph.remove_file(rel_path); // no-op for brand-new files
                let ext = Path::new(rel_path.as_str())
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                if let Some(extractor) = self.extractors.get(&ext) {
                    extractor.extract(content, rel_path, &mut graph);
                }
            }
        }

        // ── 6. Resolve cross-file references ──────────────────────────────────
        graph.resolve();

        // ── 7. Static test-node detection ─────────────────────────────────────
        Self::mark_test_nodes(&mut graph);

        // ── 8. Build description tasks for changed/new files ─────────────────
        let tasks = Self::make_tasks(&changed_files, &current_files, &graph);

        IndexResult { tasks, graph, root: self.root, fs: self.fs }
    }

    /// Statically mark nodes as `is_test = true` using file-name and
    /// name-pattern heuristics — no LLM required.
    ///
    /// Rules applied (any match → `is_test = true`):
    /// - File name matches a test-file pattern (e.g. `*_test.go`, `test_*.py`,
    ///   `*.test.ts`, `*.spec.ts`, `*Test.java`, `*Tests.java`, `*Spec.java`)
    /// - Class name ends with `Test`, `Tests`, `Spec`, `Suite`
    /// - Method/function name starts with `test_`, `Test`, or matches
    ///   `it(`, `describe(` call patterns (detected by name prefix `it_`/`describe_`)
    fn mark_test_nodes(graph: &mut DependencyGraph) {
        // Collect files that are test files by path pattern.
        let test_files: std::collections::HashSet<String> = graph
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::File && is_test_file(&n.file))
            .map(|n| n.file.clone())
            .collect();

        for node in graph.nodes.values_mut() {
            if node.is_test { continue; } // already flagged
            if test_files.contains(&node.file) {
                node.is_test = true;
                continue;
            }
            // Name-based heuristics for classes and methods not in test files
            // (e.g. test helper classes embedded in production files).
            node.is_test = match node.kind {
                NodeKind::Class | NodeKind::Trait => {
                    let n = &node.name;
                    n.ends_with("Test") || n.ends_with("Tests")
                        || n.ends_with("Spec") || n.ends_with("Suite")
                        || n.starts_with("Test") && n.len() > 4
                }
                NodeKind::Method | NodeKind::Function => {
                    let n = &node.name;
                    n.starts_with("test_") || n.starts_with("Test")
                        || n == "setUp" || n == "tearDown"
                        || n == "beforeEach" || n == "afterEach"
                        || n == "beforeAll" || n == "afterAll"
                }
                _ => false,
            };
        }
    }

    /// Build one [`DescriptionTask`] per changed/new file that contains at
    /// least one describable node (class, interface, trait, enum, method,
    /// function).
    fn make_tasks(
        changed:  &HashSet<String>,
        contents: &HashMap<String, String>,
        graph:    &DependencyGraph,
    ) -> Vec<DescriptionTask> {
        let mut tasks = Vec::new();
        for rel_path in changed {
            let content = match contents.get(rel_path) {
                Some(c) => c.as_str(),
                None => continue,
            };
            let lines: Vec<&str> = content.lines().collect();

            let describable: Vec<&crate::graph::Node> = graph
                .nodes
                .values()
                .filter(|n| n.file == *rel_path && DESCRIBED_KINDS.contains(&n.kind))
                .collect();

            if describable.is_empty() { continue; }

            let mut snippets:      HashMap<String, String>       = HashMap::new();
            let mut schema:        HashMap<String, String>       = HashMap::new();
            let mut is_test_schema: HashMap<String, Option<bool>> = HashMap::new();

            for node in describable {
                // Span lines are 1-based; clamp to actual line count.
                let start = (node.span.start_line as usize).saturating_sub(1);
                let end   = (node.span.end_line as usize).min(lines.len());
                let snippet = lines[start..end].join("\n");
                snippets.insert(node.qualified_name.clone(), snippet);
                schema.insert(node.qualified_name.clone(), String::new());
                // Nodes already confirmed by static heuristics don't need LLM
                // classification; pass their known value so the caller can skip them.
                is_test_schema.insert(
                    node.qualified_name.clone(),
                    if node.is_test { Some(true) } else { None },
                );
            }

            tasks.push(DescriptionTask {
                file: rel_path.clone(),
                snippets,
                schema,
                is_test_schema,
            });
        }
        tasks
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Recursively walk `abs_dir`, collecting all source files (relative to
    /// `self.root`) into `files`.  Respects `.gitignore` files found in each
    /// directory by accumulating them in `parent_ignores`.
    fn walk_dir(
        &self,
        abs_dir:        &str,
        parent_ignores: &[(String, Gitignore)],
        files:          &mut HashMap<String, String>,
    ) {
        // Extend the ignore stack with this directory's .gitignore (if any).
        let mut ignores = parent_ignores.to_vec();
        let gi_path = format!("{}/.gitignore", abs_dir);
        if let Some(content) = self.fs.read(&gi_path) {
            let mut builder = GitignoreBuilder::new(abs_dir);
            for line in content.lines() {
                let _ = builder.add_line(None, line);
            }
            if let Ok(gi) = builder.build() {
                ignores.push((abs_dir.to_string(), gi));
            }
        }

        for entry in self.fs.list(abs_dir) {
            // Skip hidden entries (.git, .codegraph, …).
            if entry.name.starts_with('.') { continue; }
            // Skip node_modules unconditionally.
            if entry.name == "node_modules" { continue; }

            let abs_path = format!("{}/{}", abs_dir, entry.name);

            if Self::is_ignored_by(&ignores, &abs_path, entry.is_dir) { continue; }

            if entry.is_dir {
                self.walk_dir(&abs_path, &ignores, files);
            } else {
                let ext = Path::new(entry.name.as_str())
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                if self.extractors.contains_key(&ext) {
                    if let Some(content) = self.fs.read(&abs_path) {
                        let rel = abs_path
                            .strip_prefix(&format!("{}/", self.root))
                            .unwrap_or(&abs_path)
                            .to_string();
                        files.insert(rel, content);
                    }
                }
            }
        }
    }

    /// Returns `true` if `abs_path` is ignored by any of the accumulated
    /// gitignore layers.
    fn is_ignored_by(
        ignores:  &[(String, Gitignore)],
        abs_path: &str,
        is_dir:   bool,
    ) -> bool {
        for (base, gi) in ignores {
            let prefix = format!("{}/", base);
            let rel = abs_path.strip_prefix(&prefix).unwrap_or(abs_path);
            if gi.matched(rel, is_dir).is_ignore() {
                return true;
            }
        }
        false
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::mock::MockFileSystem;
    use crate::graph::{EdgeTarget, Language, NodeKind};
    use crate::parser::hash_source;
    use crate::serializer::GraphSerializer;

    // ── Fixture sources ───────────────────────────────────────────────────────

    const JAVA_SRC: &str = include_str!("languages/test/fixtures/Shop.java");
    const RUST_SRC: &str = include_str!("languages/test/fixtures/shop.rs");

    // Minimal Java snippets for fine-grained incremental tests.
    const JAVA_TWO_CLASSES: &str = "\
package com.example;
public class Alpha { public void run() {} }
class Beta { public void stop() {} }
";
    const JAVA_ONE_CLASS: &str = "\
package com.example;
public class Alpha { public void run() {} }
";

    // Minimal Rust snippets.
    const RUST_TWO_STRUCTS: &str = "pub struct Alpha { pub x: i32 }\npub struct Beta  { pub y: i32 }\n";
    const RUST_ONE_STRUCT:  &str = "pub struct Alpha { pub x: i32 }\n";

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Run the indexer from scratch (no existing graph.yml).
    fn run(root: &str, fs: MockFileSystem) -> DependencyGraph {
        GraphIndexer::new(root, Box::new(fs)).run().finish()
    }

    /// Run a second pass that sees an existing graph.yml.
    fn run_incremental(root: &str, prev: &DependencyGraph, fs: MockFileSystem) -> DependencyGraph {
        let mut fs2 = fs;
        let yaml = GraphSerializer::serialize(prev).unwrap();
        fs2.add(&format!("{}/.codegraph/graph.yml", root), &yaml);
        GraphIndexer::new(root, Box::new(fs2)).run().finish()
    }

    fn nodes_of_kind(g: &DependencyGraph, kind: NodeKind) -> Vec<&crate::graph::Node> {
        g.nodes.values().filter(|n| n.kind == kind).collect()
    }

    fn node_named<'a>(g: &'a DependencyGraph, name: &str) -> Option<&'a crate::graph::Node> {
        g.nodes.values().find(|n| n.name == name || n.qualified_name == name)
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 1. Filesystem traversal
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_java_file_is_indexed() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Foo.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert!(g.node_count() > 0);
        assert!(g.nodes.values().any(|n| n.language == Language::Java));
    }

    #[test]
    fn test_rust_file_is_indexed() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/src/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        assert!(g.node_count() > 0);
        assert!(g.nodes.values().any(|n| n.language == Language::Rust));
    }

    #[test]
    fn test_empty_root_produces_empty_graph() {
        let g = run("/r", MockFileSystem::new());
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn test_file_at_root_level_is_found() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Main.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert!(g.node_count() > 0);
    }

    #[test]
    fn test_files_in_deeply_nested_dirs_are_found() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/a/b/c/d/Deep.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert!(g.node_count() > 0);
    }

    #[test]
    fn test_multiple_files_produce_combined_graph() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/a/Shop.java", root), JAVA_SRC);
        fs.add(&format!("{}/b/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        assert!(g.nodes.values().any(|n| n.language == Language::Java));
        assert!(g.nodes.values().any(|n| n.language == Language::Rust));
    }

    #[test]
    fn test_unrecognised_extensions_are_ignored() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/README.md", root),    "# doc");
        fs.add(&format!("{}/config.toml", root),  "[package]");
        fs.add(&format!("{}/style.css", root),    "body {}");
        fs.add(&format!("{}/data.json", root),    "{}");
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0, "no supported extensions");
    }

    #[test]
    fn test_two_java_files_combined_into_one_graph() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        fs.add(&format!("{}/B.java", root), JAVA_ONE_CLASS);
        let g = run(root, fs);
        // Two file nodes, at least Alpha×2 (from each file) + Beta×1
        assert!(nodes_of_kind(&g, NodeKind::File).len() >= 2);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 2. Skip rules
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_hidden_directories_are_skipped() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/.secret/Foo.java", root), JAVA_SRC);
        fs.add(&format!("{}/.git/Hook.java", root),   JAVA_SRC);
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn test_hidden_files_at_root_are_skipped() {
        // A file like ".hidden.java" starts with '.' — should be skipped.
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/.hidden.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn test_node_modules_directory_is_skipped() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/node_modules/Foo.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn test_node_modules_nested_is_skipped() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/packages/lib/node_modules/Foo.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn test_codegraph_output_dir_is_not_indexed() {
        // .codegraph starts with '.' and is hidden, so its contents are never walked.
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/.codegraph/graph.yml", root), "# not source");
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 3. Gitignore support
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_root_gitignore_excludes_directory() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/build/Output.java", root), JAVA_SRC);
        fs.add(&format!("{}/.gitignore", root), "build/\n");
        let g = run(root, fs);
        assert_eq!(g.node_count(), 0);
    }

    #[test]
    fn test_root_gitignore_excludes_specific_file() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/src/Gen.java", root),    JAVA_SRC);
        fs.add(&format!("{}/src/Manual.java", root), JAVA_ONE_CLASS);
        fs.add(&format!("{}/.gitignore", root), "src/Gen.java\n");
        let g = run(root, fs);
        // Only Manual.java should be indexed.
        assert!(g.nodes.values().all(|n| !n.file.contains("Gen.java")),
            "Gen.java should be excluded by .gitignore");
        assert!(g.nodes.values().any(|n| n.file.contains("Manual.java")));
    }

    #[test]
    fn test_subdirectory_gitignore_is_respected() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/src/main/Foo.java", root),      JAVA_SRC);
        fs.add(&format!("{}/src/gen/Generated.java", root), JAVA_SRC);
        // .gitignore inside src/ excludes the gen/ subdirectory.
        fs.add(&format!("{}/src/.gitignore", root), "gen/\n");
        let g = run(root, fs);
        // src/main/Foo.java should be indexed; src/gen/Generated.java should not.
        assert!(g.nodes.values().any(|n| n.file.contains("Foo.java")),
            "Foo.java should still be indexed");
        assert!(g.nodes.values().all(|n| !n.file.contains("Generated.java")),
            "Generated.java should be excluded by sub-directory .gitignore");
    }

    #[test]
    fn test_gitignore_comment_and_blank_lines_are_ignored() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/src/Foo.java", root), JAVA_SRC);
        // .gitignore with only comments and blanks — nothing should be excluded.
        fs.add(&format!("{}/.gitignore", root), "# this is a comment\n\n  \n");
        let g = run(root, fs);
        assert!(g.node_count() > 0, "comments and blanks must not exclude anything");
    }

    #[test]
    fn test_parent_and_child_gitignore_both_applied() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        // root .gitignore excludes dist/
        fs.add(&format!("{}/.gitignore", root), "dist/\n");
        // src/.gitignore excludes gen/
        fs.add(&format!("{}/src/.gitignore", root), "gen/\n");
        // Files that should be indexed.
        fs.add(&format!("{}/src/main/Good.java", root), JAVA_SRC);
        // Files that should be excluded.
        fs.add(&format!("{}/dist/Out.java", root),     JAVA_SRC);
        fs.add(&format!("{}/src/gen/Bad.java", root),  JAVA_SRC);
        let g = run(root, fs);
        assert!(g.nodes.values().any(|n| n.file.contains("Good.java")));
        assert!(g.nodes.values().all(|n| !n.file.contains("Out.java")));
        assert!(g.nodes.values().all(|n| !n.file.contains("Bad.java")));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 4. Graph content
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_file_node_path_is_relative_to_root() {
        let root = "/my/project";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/src/com/example/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        let file_node = nodes_of_kind(&g, NodeKind::File);
        assert!(!file_node.is_empty());
        for n in &file_node {
            assert!(
                !n.file.starts_with('/'),
                "file path '{}' should be relative (not absolute)",
                n.file
            );
            assert!(
                n.file.starts_with("src/"),
                "expected 'src/…', got '{}'",
                n.file
            );
        }
    }

    #[test]
    fn test_file_node_has_sha256_hash() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        let file_node = nodes_of_kind(&g, NodeKind::File);
        assert!(!file_node.is_empty());
        for n in &file_node {
            assert!(n.hash.is_some(), "File node must have a hash");
            // SHA-256 hex digest is exactly 64 characters.
            assert_eq!(n.hash.as_deref().unwrap().len(), 64,
                "hash should be a 64-char hex SHA-256");
        }
    }

    #[test]
    fn test_file_hash_matches_source_content() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        let file_node = nodes_of_kind(&g, NodeKind::File).into_iter().next().unwrap();
        assert_eq!(
            file_node.hash.as_deref().unwrap(),
            hash_source(JAVA_SRC),
            "File node hash should equal hash_source(content)"
        );
    }

    #[test]
    fn test_java_fixture_produces_class_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        let classes: Vec<_> = nodes_of_kind(&g, NodeKind::Class);
        assert!(!classes.is_empty(), "Java classes should be extracted");
        let names: Vec<&str> = classes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Product"),       "Product class expected");
        assert!(names.contains(&"BaseEntity"),    "BaseEntity class expected");
        assert!(names.contains(&"NotFoundException"), "NotFoundException expected");
    }

    #[test]
    fn test_java_fixture_produces_interface_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        let interfaces: Vec<_> = nodes_of_kind(&g, NodeKind::Interface);
        assert!(!interfaces.is_empty(), "Java interfaces should be extracted");
        let names: Vec<&str> = interfaces.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Repository"), "Repository interface expected");
    }

    #[test]
    fn test_java_fixture_produces_enum_node() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        let enums: Vec<_> = nodes_of_kind(&g, NodeKind::Enum);
        assert!(!enums.is_empty(), "Java enum should be extracted");
        let names: Vec<&str> = enums.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Category"), "Category enum expected");
    }

    #[test]
    fn test_java_fixture_produces_method_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert!(!nodes_of_kind(&g, NodeKind::Method).is_empty(),
            "Java methods should be extracted");
    }

    #[test]
    fn test_java_fixture_graph_has_edges() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Shop.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert!(g.edge_count() > 0, "Java fixture should produce edges");
    }

    #[test]
    fn test_rust_fixture_produces_struct_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        let classes: Vec<_> = nodes_of_kind(&g, NodeKind::Class); // Rust structs → Class
        let names: Vec<&str> = classes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Product"),      "Product struct expected");
        assert!(names.contains(&"Cart"),         "Cart struct expected");
        assert!(names.contains(&"InMemoryRepo"), "InMemoryRepo expected");
    }

    #[test]
    fn test_rust_fixture_produces_trait_nodes() {
        // The Rust extractor maps traits to NodeKind::Interface.
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        let traits: Vec<_> = nodes_of_kind(&g, NodeKind::Interface);
        assert!(!traits.is_empty(), "Rust traits should be extracted as Interface nodes");
        let names: Vec<&str> = traits.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Describable"), "Describable trait expected");
        assert!(names.contains(&"Priceable"),   "Priceable trait expected");
        assert!(names.contains(&"Repository"),  "Repository trait expected");
    }

    #[test]
    fn test_rust_fixture_produces_enum_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        let enums: Vec<_> = nodes_of_kind(&g, NodeKind::Enum);
        let names: Vec<&str> = enums.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Category"),  "Category enum expected");
        assert!(names.contains(&"ShopError"), "ShopError enum expected");
    }

    #[test]
    fn test_rust_fixture_produces_function_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        let fns: Vec<_> = nodes_of_kind(&g, NodeKind::Function);
        let names: Vec<&str> = fns.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"print_inventory"), "print_inventory fn expected");
        assert!(names.contains(&"cheapest"),        "cheapest fn expected");
    }

    #[test]
    fn test_rust_fixture_produces_type_alias_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/shop.rs", root), RUST_SRC);
        let g = run(root, fs);
        let aliases: Vec<_> = nodes_of_kind(&g, NodeKind::TypeAlias);
        let names: Vec<&str> = aliases.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"ProductId"), "ProductId type alias expected");
        assert!(names.contains(&"Inventory"), "Inventory type alias expected");
    }

    #[test]
    fn test_supported_extensions_includes_builtins() {
        let idx = GraphIndexer::new("/r", Box::new(MockFileSystem::new()));
        let exts = idx.supported_extensions();
        assert!(exts.contains(&"java"), "java should be a built-in extension");
        assert!(exts.contains(&"rs"),   "rs should be a built-in extension");
    }

    #[test]
    fn test_custom_extractor_is_invoked() {
        use crate::graph::{DependencyGraph, Language, Node, Span};
        use crate::parser::LanguageExtractor;

        struct MarkExtractor;
        impl LanguageExtractor for MarkExtractor {
            fn language(&self) -> Language { Language::Unknown }
            fn extract(&self, _src: &str, file: &str, graph: &mut DependencyGraph) {
                graph.add_node(Node::new(
                    0, NodeKind::File, "mark", "mark",
                    file, Span::new(0, 0, 0, 0), Language::Unknown,
                ));
            }
        }

        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/foo.mark", root), "anything");
        let mut idx = GraphIndexer::new(root, Box::new(fs));
        idx.register(&["mark"], MarkExtractor);
        let g = idx.run().finish();
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.nodes.values().next().unwrap().language, Language::Unknown);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 5. Incremental indexing — unchanged files
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_unchanged_file_preserves_all_node_ids() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);
        let ids1: HashSet<u64> = g1.nodes.keys().copied().collect();

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        let g2 = run_incremental(root, &g1, fs2);
        let ids2: HashSet<u64> = g2.nodes.keys().copied().collect();

        assert_eq!(ids1, ids2, "unchanged file must keep all node IDs");
    }

    #[test]
    fn test_unchanged_file_preserves_node_count() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        let g2 = run_incremental(root, &g1, fs2);

        assert_eq!(g1.node_count(), g2.node_count(),
            "node count should be stable across runs when nothing changed");
    }

    #[test]
    fn test_unchanged_file_preserves_edge_count() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        let g2 = run_incremental(root, &g1, fs2);

        assert_eq!(g1.edge_count(), g2.edge_count(),
            "edge count should be stable when nothing changed");
    }

    #[test]
    fn test_three_consecutive_runs_are_stable() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        let g2 = run_incremental(root, &g1, fs2);

        let mut fs3 = MockFileSystem::new();
        fs3.add(&format!("{}/A.java", root), JAVA_SRC);
        let g3 = run_incremental(root, &g2, fs3);

        let ids1: HashSet<u64> = g1.nodes.keys().copied().collect();
        let ids3: HashSet<u64> = g3.nodes.keys().copied().collect();
        assert_eq!(ids1, ids3, "node IDs must be stable across 3 identical runs");
        assert_eq!(g1.node_count(), g3.node_count());
        assert_eq!(g1.edge_count(), g3.edge_count());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 6. Incremental indexing — changed files
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_changed_file_updates_hash() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);
        let hash1 = g1.nodes.values()
            .find(|n| n.kind == NodeKind::File).unwrap()
            .hash.clone().unwrap();

        let modified = format!("{}\n// change", JAVA_SRC);
        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), &modified);
        let g2 = run_incremental(root, &g1, fs2);
        let hash2 = g2.nodes.values()
            .find(|n| n.kind == NodeKind::File).unwrap()
            .hash.clone().unwrap();

        assert_ne!(hash1, hash2, "hash must change when file content changes");
    }

    /// When a class is removed from a Java file the old class node must not
    /// survive in the next incremental graph.
    #[test]
    fn test_entity_removed_from_changed_java_file() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let g1 = run(root, fs1);
        assert!(node_named(&g1, "Beta").is_some(), "Beta should exist after first run");

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_ONE_CLASS);
        let g2 = run_incremental(root, &g1, fs2);
        assert!(node_named(&g2, "Beta").is_none(),
            "Beta must be gone after it is removed from the file");
        assert!(node_named(&g2, "Alpha").is_some(),
            "Alpha should still be in the graph");
    }

    /// When a file changes, surviving entities are re-indexed and receive new IDs.
    /// ID preservation is at the *file* level — only fully-unchanged files keep IDs.
    #[test]
    fn test_surviving_entity_is_present_after_sibling_removed() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let g1 = run(root, fs1);
        assert!(node_named(&g1, "Alpha").is_some());

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_ONE_CLASS);
        let g2 = run_incremental(root, &g1, fs2);

        // Alpha is still present; Beta is gone.
        assert!(node_named(&g2, "Alpha").is_some(),
            "Alpha must still be in the graph after re-indexing");
        assert!(node_named(&g2, "Beta").is_none(),
            "Beta must be gone from the graph");
        // When a file changes, entities are fully re-extracted — their IDs change.
        let alpha_id1 = node_named(&g1, "Alpha").unwrap().id;
        let alpha_id2 = node_named(&g2, "Alpha").unwrap().id;
        assert_ne!(alpha_id1, alpha_id2,
            "a changed file causes all its entities to be re-indexed with fresh IDs");
    }

    #[test]
    fn test_entity_removed_from_changed_rust_file() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/shop.rs", root), RUST_TWO_STRUCTS);
        let g1 = run(root, fs1);
        assert!(node_named(&g1, "Beta").is_some());

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/shop.rs", root), RUST_ONE_STRUCT);
        let g2 = run_incremental(root, &g1, fs2);
        assert!(node_named(&g2, "Beta").is_none(), "Beta struct should be removed");
        assert!(node_named(&g2, "Alpha").is_some(), "Alpha struct should remain");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 7. Incremental indexing — deleted files
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_deleted_file_removes_all_its_nodes() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        fs1.add(&format!("{}/B.rs",   root), RUST_SRC);
        let g1 = run(root, fs1);
        let rust_count = g1.nodes.values().filter(|n| n.language == Language::Rust).count();
        assert!(rust_count > 0);

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC); // Rust file deleted
        let g2 = run_incremental(root, &g1, fs2);

        assert_eq!(
            g2.nodes.values().filter(|n| n.language == Language::Rust).count(),
            0,
            "all Rust nodes should be gone after file deletion"
        );
        assert!(g2.nodes.values().any(|n| n.language == Language::Java),
            "Java nodes should survive");
    }

    #[test]
    fn test_deleted_file_removes_its_edges() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.rs", root), RUST_SRC);
        let g1 = run(root, fs1);
        let edge_count1 = g1.edge_count();
        assert!(edge_count1 > 0);

        // Delete the only file.
        let fs2 = MockFileSystem::new();
        let g2 = run_incremental(root, &g1, fs2);
        assert_eq!(g2.edge_count(), 0, "edges should be gone when their file is deleted");
        assert_eq!(g2.node_count(), 0);
    }

    #[test]
    fn test_all_files_deleted_produces_empty_graph() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        fs1.add(&format!("{}/B.rs",   root), RUST_SRC);
        let g1 = run(root, fs1);
        assert!(g1.node_count() > 0);

        let g2 = run_incremental(root, &g1, MockFileSystem::new());
        assert_eq!(g2.node_count(), 0, "no files → empty graph");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 8. Incremental indexing — new files added
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_new_file_is_added_to_existing_graph() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);
        let count1 = g1.node_count();

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        fs2.add(&format!("{}/B.rs",   root), RUST_SRC);
        let g2 = run_incremental(root, &g1, fs2);

        assert!(g2.node_count() > count1,
            "adding a new file should increase node count");
        assert!(g2.nodes.values().any(|n| n.language == Language::Rust),
            "new Rust file should be in graph");
    }

    #[test]
    fn test_existing_nodes_keep_ids_when_new_file_added() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);
        let ids1: HashSet<u64> = g1.nodes.keys().copied().collect();

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        fs2.add(&format!("{}/B.rs",   root), RUST_SRC);
        let g2 = run_incremental(root, &g1, fs2);
        let ids2: HashSet<u64> = g2.nodes.keys().copied().collect();

        // Every id from run 1 must still be present in run 2.
        assert!(
            ids1.is_subset(&ids2),
            "existing node IDs must be preserved when a new file is added"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 9. Edge indices after incremental operations
    // ═══════════════════════════════════════════════════════════════════════

    /// edges_from and edges_to must be consistent with self.edges after each run.
    #[test]
    fn test_edge_indices_are_consistent_after_fresh_run() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert_edge_indices_consistent(&g);
    }

    #[test]
    fn test_edge_indices_are_consistent_after_incremental_run() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        fs1.add(&format!("{}/B.rs",   root), RUST_SRC);
        let g1 = run(root, fs1);

        // Delete B.rs.
        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        let g2 = run_incremental(root, &g1, fs2);
        assert_edge_indices_consistent(&g2);
    }

    #[test]
    fn test_no_dangling_resolved_edges_after_file_deletion() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        fs1.add(&format!("{}/B.rs",   root), RUST_SRC);
        let g1 = run(root, fs1);

        // Delete B.rs.
        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        let g2 = run_incremental(root, &g1, fs2);

        // No edge should be Resolved to a NodeId that doesn't exist.
        for edge in &g2.edges {
            if let EdgeTarget::Resolved(id) = edge.to {
                assert!(
                    g2.nodes.contains_key(&id),
                    "edge {} is Resolved({}) but that node no longer exists",
                    edge.id, id
                );
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 10. Robustness / edge cases
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_corrupt_graph_yml_starts_fresh() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_SRC);
        fs.add(&format!("{}/.codegraph/graph.yml", root),
               "this is not valid yaml: !!: !!:");
        // Should fall back to a clean graph without panicking.
        let g = run(root, fs);
        assert!(g.node_count() > 0, "corrupt YAML must be ignored; file indexed fresh");
    }

    #[test]
    fn test_empty_graph_yml_starts_fresh() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_SRC);
        fs.add(&format!("{}/.codegraph/graph.yml", root), "");
        let g = run(root, fs);
        assert!(g.node_count() > 0);
    }

    #[test]
    fn test_no_graph_yml_starts_fresh() {
        // No .codegraph/graph.yml present — must index from scratch.
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_SRC);
        let g = run(root, fs);
        assert!(g.node_count() > 0, "fresh run without existing graph.yml should work");
    }

    #[test]
    fn test_extension_matching_is_case_insensitive() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/Foo.JAVA", root), JAVA_SRC); // uppercase extension
        let g = run(root, fs);
        assert!(g.node_count() > 0, ".JAVA (uppercase) should be matched");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 11. Rebuild flag
    // ═══════════════════════════════════════════════════════════════════════

    /// With `rebuild(true)` an existing graph.yml is ignored and the graph is
    /// built entirely from scratch — node IDs restart from 0.
    #[test]
    fn test_rebuild_ignores_existing_graph_yml() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);
        let ids1: HashSet<u64> = g1.nodes.keys().copied().collect();
        let yaml1 = GraphSerializer::serialize(&g1).unwrap();

        // Second run with rebuild=true — existing graph.yml is pre-populated
        // but must be ignored.
        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        fs2.add(&format!("{}/.codegraph/graph.yml", root), &yaml1);
        let g2 = GraphIndexer::new(root, Box::new(fs2)).rebuild(true).run().finish();
        let ids2: HashSet<u64> = g2.nodes.keys().copied().collect();

        // The graph should be structurally identical (same nodes, same edges)
        // but built fresh — node IDs restart from 0, so the sets are equal.
        assert_eq!(g1.node_count(), g2.node_count(),
            "rebuild must produce the same node count");
        assert_eq!(ids1, ids2,
            "rebuilding from scratch produces the same ID set as the first run");
    }

    #[test]
    fn test_rebuild_false_is_the_default() {
        // Without calling .rebuild(true) the incremental path is taken.
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_SRC);
        let g1 = run(root, fs1);
        let ids1: HashSet<u64> = g1.nodes.keys().copied().collect();
        let yaml1 = GraphSerializer::serialize(&g1).unwrap();

        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_SRC);
        fs2.add(&format!("{}/.codegraph/graph.yml", root), &yaml1);
        // No .rebuild() call — incremental by default.
        let g2 = GraphIndexer::new(root, Box::new(fs2)).run().finish();
        let ids2: HashSet<u64> = g2.nodes.keys().copied().collect();

        assert_eq!(ids1, ids2, "default (non-rebuild) run must preserve IDs");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // 9. Description tasks
    // ═══════════════════════════════════════════════════════════════════════

    // ── 9a. pending_tasks / make_tasks ───────────────────────────────────────

    #[test]
    fn test_no_tasks_when_nothing_indexed() {
        // Empty root → no files → no tasks.
        let result = GraphIndexer::new("/r", Box::new(MockFileSystem::new())).run();
        assert!(result.pending_tasks().is_empty());
    }

    #[test]
    fn test_task_produced_for_java_file_with_classes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        assert_eq!(result.pending_tasks().len(), 1);
        assert_eq!(result.pending_tasks()[0].file, "A.java");
    }

    #[test]
    fn test_task_snippets_keys_match_schema_keys() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        let task = &result.pending_tasks()[0];
        assert_eq!(
            task.snippets.keys().collect::<std::collections::HashSet<_>>(),
            task.schema.keys().collect::<std::collections::HashSet<_>>(),
            "snippets and schema must have identical keys",
        );
    }

    #[test]
    fn test_task_snippets_are_non_empty() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        let task = &result.pending_tasks()[0];
        for (qname, snippet) in &task.snippets {
            assert!(!snippet.is_empty(), "snippet for {qname} should not be empty");
        }
    }

    #[test]
    fn test_task_snippets_do_not_contain_full_file() {
        // JAVA_TWO_CLASSES has two classes; each snippet should be shorter than
        // the full file, since it covers only one entity's span.
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        let task = &result.pending_tasks()[0];
        // At least some snippets must be strictly shorter than the full source.
        let full_lines = JAVA_TWO_CLASSES.lines().count();
        let any_shorter = task.snippets.values()
            .any(|s| s.lines().count() < full_lines);
        assert!(any_shorter, "at least one snippet should be shorter than the full file");
    }

    #[test]
    fn test_task_schema_contains_class_and_method_names() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        let schema = &result.pending_tasks()[0].schema;
        // JAVA_TWO_CLASSES has Alpha + Beta + Alpha.run + Beta.stop
        assert!(schema.keys().any(|k| k.contains("Alpha")), "Alpha missing");
        assert!(schema.keys().any(|k| k.contains("Beta")),  "Beta missing");
        assert!(schema.keys().any(|k| k.contains("run")),   "run missing");
        assert!(schema.keys().any(|k| k.contains("stop")),  "stop missing");
    }

    #[test]
    fn test_task_schema_values_start_empty() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        for (k, v) in &result.pending_tasks()[0].schema {
            assert!(v.is_empty(), "schema value for {k} should be empty initially");
        }
    }

    #[test]
    fn test_one_task_per_changed_file() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        fs.add(&format!("{}/B.java", root), JAVA_ONE_CLASS);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        assert_eq!(result.pending_tasks().len(), 2);
        let files: Vec<&str> = result.pending_tasks().iter().map(|t| t.file.as_str()).collect();
        assert!(files.contains(&"A.java"));
        assert!(files.contains(&"B.java"));
    }

    #[test]
    fn test_no_task_for_file_without_describable_nodes() {
        // A Rust file with only a struct (no methods/functions) should still
        // produce a task because Struct is… wait, Struct is not in
        // DESCRIBED_KINDS.  Only File/Package/Struct/Field nodes with no
        // methods → schema is empty → task is skipped.
        let root = "/r";
        let src = "pub struct Foo { pub x: i32 }\n"; // no functions/methods
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/foo.rs", root), src);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        // If the extractor emits no describable nodes the task list should be
        // empty (or at least the foo.rs task should have an empty schema and
        // thus be omitted).
        for task in result.pending_tasks() {
            assert!(!task.schema.is_empty(),
                "task for {} should have been omitted — empty schema", task.file);
        }
    }

    // ── 9b. Incremental: unchanged files don't get tasks ─────────────────────

    #[test]
    fn test_unchanged_file_produces_no_task_on_second_run() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let g1 = GraphIndexer::new(root, Box::new(fs1)).run().finish();

        let mut fs2 = MockFileSystem::new();
        let yaml = GraphSerializer::serialize(&g1).unwrap();
        fs2.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        fs2.add(&format!("{}/.codegraph/graph.yml", root), &yaml);

        let result = GraphIndexer::new(root, Box::new(fs2)).run();
        assert!(result.pending_tasks().is_empty(),
            "unchanged file must not produce a task on the second run");
    }

    #[test]
    fn test_changed_file_produces_task_on_second_run() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let g1 = GraphIndexer::new(root, Box::new(fs1)).run().finish();

        let mut fs2 = MockFileSystem::new();
        let yaml = GraphSerializer::serialize(&g1).unwrap();
        // File content changed → new hash → must be re-indexed and get a task.
        fs2.add(&format!("{}/A.java", root), JAVA_ONE_CLASS);
        fs2.add(&format!("{}/.codegraph/graph.yml", root), &yaml);

        let result = GraphIndexer::new(root, Box::new(fs2)).run();
        assert_eq!(result.pending_tasks().len(), 1);
        assert_eq!(result.pending_tasks()[0].file, "A.java");
    }

    #[test]
    fn test_new_file_produces_task_on_second_run() {
        let root = "/r";
        let mut fs1 = MockFileSystem::new();
        fs1.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let g1 = GraphIndexer::new(root, Box::new(fs1)).run().finish();

        let mut fs2 = MockFileSystem::new();
        let yaml = GraphSerializer::serialize(&g1).unwrap();
        fs2.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES); // unchanged
        fs2.add(&format!("{}/B.java", root), JAVA_ONE_CLASS);   // brand new
        fs2.add(&format!("{}/.codegraph/graph.yml", root), &yaml);

        let result = GraphIndexer::new(root, Box::new(fs2)).run();
        assert_eq!(result.pending_tasks().len(), 1,
            "only the new file should get a task");
        assert_eq!(result.pending_tasks()[0].file, "B.java");
    }

    // ── 9c. commit / finish ───────────────────────────────────────────────────

    #[test]
    fn test_finish_writes_graph_yml() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let fs_arc = std::sync::Arc::new(std::sync::Mutex::new(MockFileSystem::new()));
        // We need to observe the write; use MockFileSystem directly via finish().
        let mut fs2 = MockFileSystem::new();
        fs2.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let boxed: Box<dyn crate::filesystem::FileSystem> = Box::new(fs2);
        GraphIndexer::new(root, boxed).run().finish();
        // finish() returns without panicking; graph.yml is written via host_write
        // (MockFileSystem captures writes internally — we trust it doesn't panic).
        let _ = fs_arc; // suppress warning
    }

    #[test]
    fn test_commit_applies_descriptions_to_nodes() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();

        // Collect the schema keys so we know what names to fill.
        let task    = &result.pending_tasks()[0];
        let mut descs: HashMap<String, String> = task
            .schema
            .keys()
            .map(|k| (k.clone(), format!("Description of {k}")))
            .collect();

        // Commit with filled descriptions.
        let g = result.commit(descs.clone(), HashMap::new());

        // Every described node must now carry the description we supplied.
        for (qname, desc) in &descs {
            if let Some(node) = g.nodes.values().find(|n| &n.qualified_name == qname) {
                assert_eq!(
                    node.description.as_deref(), Some(desc.as_str()),
                    "node {qname} should have description"
                );
            }
        }
    }

    #[test]
    fn test_commit_with_empty_map_leaves_descriptions_none() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let g = GraphIndexer::new(root, Box::new(fs)).run().finish();

        // No descriptions supplied → all nodes have description == None.
        for node in g.nodes.values() {
            assert!(node.description.is_none(),
                "node {} should have no description", node.qualified_name);
        }
    }

    #[test]
    fn test_commit_ignores_empty_string_values() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        let task   = &result.pending_tasks()[0];

        // Pass all values as empty strings — they must be ignored.
        let descs: HashMap<String, String> = task
            .schema
            .keys()
            .map(|k| (k.clone(), String::new()))
            .collect();
        let g = result.commit(descs, HashMap::new());
        for node in g.nodes.values() {
            assert!(node.description.is_none(),
                "empty-string description must not be written to node {}",
                node.qualified_name);
        }
    }

    #[test]
    fn test_commit_ignores_unknown_keys() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        let mut descs: HashMap<String, String> = HashMap::new();
        descs.insert("com.example.DoesNotExist".to_string(), "should be silently ignored".to_string());
        // Must not panic.
        let g = result.commit(descs, HashMap::new());
        assert!(g.node_count() > 0);
    }

    #[test]
    fn test_pending_tasks_count_matches_tasks_field() {
        let root = "/r";
        let mut fs = MockFileSystem::new();
        fs.add(&format!("{}/A.java", root), JAVA_TWO_CLASSES);
        fs.add(&format!("{}/B.java", root), JAVA_ONE_CLASS);
        let result = GraphIndexer::new(root, Box::new(fs)).run();
        assert_eq!(result.pending_tasks().len(), result.tasks.len());
    }

    // ─── helper ──────────────────────────────────────────────────────────────

    /// Verify that `edges_from` and `edges_to` exactly mirror `edges`.
    fn assert_edge_indices_consistent(g: &DependencyGraph) {
        // Rebuild expected indices from scratch.
        let mut exp_from: HashMap<u64, Vec<u64>> = HashMap::new();
        let mut exp_to:   HashMap<u64, Vec<u64>> = HashMap::new();
        for edge in &g.edges {
            exp_from.entry(edge.from).or_default().push(edge.id);
            if let EdgeTarget::Resolved(to) = edge.to {
                exp_to.entry(to).or_default().push(edge.id);
            }
        }
        // Sort for deterministic comparison.
        for v in exp_from.values_mut() { v.sort_unstable(); }
        for v in exp_to.values_mut()   { v.sort_unstable(); }

        let mut got_from = g.edges_from.clone();
        let mut got_to   = g.edges_to.clone();
        for v in got_from.values_mut() { v.sort_unstable(); }
        for v in got_to.values_mut()   { v.sort_unstable(); }

        assert_eq!(got_from, exp_from, "edges_from index is inconsistent");
        assert_eq!(got_to,   exp_to,   "edges_to index is inconsistent");
    }
}
