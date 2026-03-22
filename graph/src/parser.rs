use sha2::{Digest, Sha256};

use crate::graph::{DependencyGraph, Language};

/// Compute the SHA-256 hex digest of `source`.
/// Used by extractors to populate `Node::hash` on `File` nodes.
pub fn hash_source(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    hex::encode(digest)
}

/// Implemented once per supported language.
/// Each extractor:
///  1. Parses `source` with tree-sitter using the appropriate grammar.
///  2. Runs S-expression queries to map syntax nodes → graph nodes/edges.
///  3. Mutates `graph` in place (node ids are assigned by the graph).
///
/// Edge targets may be left as `EdgeTarget::Unresolved` after extraction;
/// call `graph.resolve()` once all files have been processed.
pub trait LanguageExtractor {
    /// The language this extractor handles.
    fn language(&self) -> Language;

    /// Extract all nodes and edges from `source` and add them to `graph`.
    ///
    /// * `source` — UTF-8 source text.
    /// * `file`   — canonical file path (used as the node's `file` field
    ///              and as the key in `graph.by_file`).
    fn extract(&self, source: &str, file: &str, graph: &mut DependencyGraph);
}
