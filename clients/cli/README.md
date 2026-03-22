# @codegraph/cli

Command-line interface for **codegraph** — indexes source code into a
language-agnostic dependency graph stored as `<project>/.codegraph/graph.yml`.

## Requirements

- Node.js ≥ 18
- The `codegraph.wasm` binary (built from `graph/`)

## Installation

```bash
npm install -g @codegraph/cli
```

Or run directly from the repo:

```bash
cd clients/cli
npm install
npm run build
node dist/index.js <command>
```

## Building the WASM binary

The CLI delegates all indexing work to a WASM binary compiled from the Rust
crate in `graph/`.  Build it once before running the CLI:

```bash
cd graph
cargo build --release
```

The binary is expected at:

```
graph/target/wasm32-wasip1/release/codegraph.wasm
```

Override the path at runtime with the `CODEGRAPH_WASM` environment variable:

```bash
CODEGRAPH_WASM=/custom/path/codegraph.wasm codegraph index .
```

## Commands

### `index <path>`

Scan `<path>` recursively, build or update the dependency graph, and write the
result to `<path>/.codegraph/graph.yml`.

```
codegraph index <path> [options]
```

**Options**

| Flag | Description |
|------|-------------|
| `-r, --rebuild` | Discard any existing `graph.yml` and rebuild from scratch |

**Examples**

```bash
# Incremental update (only re-indexes changed or new files)
codegraph index ./my-project

# Full rebuild (ignores existing graph.yml)
codegraph index --rebuild ./my-project
```

## How it works

### Incremental indexing (default)

On each run the indexer:

1. Loads `<path>/.codegraph/graph.yml` if it exists.
2. Computes SHA-256 hashes of all current source files.
3. Skips files whose hash has not changed — their nodes and edges are preserved
   with the same IDs.
4. Re-indexes files that changed: removes their old nodes and edges, then
   extracts fresh ones.
5. Removes nodes and edges for files that no longer exist.
6. Resolves cross-file references and writes the updated graph.

### Full rebuild (`--rebuild`)

Ignores any existing `graph.yml` and indexes every file from scratch.  Useful
after upgrading the indexer or when the graph is suspected to be stale.

### Gitignore support

The indexer respects `.gitignore` files found in every directory it traverses.
Patterns from parent directories are inherited by subdirectories.  Hidden
directories (`.git`, `.codegraph`, etc.) and `node_modules` are always skipped.

## Supported languages

| Extension | Language |
|-----------|----------|
| `.java`   | Java     |
| `.rs`     | Rust     |

## Output format

The graph is serialised as YAML at `<path>/.codegraph/graph.yml`.  It contains:

- **nodes** — source entities (files, packages, classes, interfaces, traits,
  enums, functions, methods, fields, …) with qualified names, spans, visibility,
  annotations, generic parameters, and SHA-256 hashes for file nodes.
- **edges** — typed relationships between nodes (contains, imports, extends,
  implements, calls, reads, writes, instantiates, …) with source spans.

## Development

```bash
npm run build   # compile TypeScript → dist/
npm run dev     # watch mode
```
