//!
//!
//! The binary is built for `wasm32-wasip1` in **reactor** mode.
//! TypeScript initialises it with `wasi.initialize(instance)` (which runs
//! `_initialize` instead of `_start`/`main`) and then drives the indexing
//! pipeline by calling the exported functions below:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │ 1. ptr = fs_response_reserve(root_len)  – allocate buffer       │
//! │    memory[ptr..ptr+root_len] = root_utf8                        │
//! │    n   = wasm_run(ptr, root_len, rebuild)  – n = task count     │
//! │                                                                 │
//! │ 2. len = wasm_pending_tasks()                                   │
//! │    ptr = wasm_response_ptr()                                    │
//! │    tasks_json = memory[ptr..ptr+len]  – JSON array              │
//! │                                                                 │
//! │ 3. (TypeScript parallelises LLM calls here)                    │
//! │                                                                 │
//! │ 4. ptr = fs_response_reserve(desc_len)  – reuse same buffer    │
//! │    memory[ptr..ptr+desc_len] = descriptions_json               │
//! │    wasm_set_descriptions(ptr, desc_len)  – writes graph.yml    │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The three host I/O imports remain unchanged:
//! ```text
//! host_list(dir_ptr, dir_len) -> u32
//! host_read(path_ptr, path_len) -> u32
//! host_write(path_ptr, path_len, content_ptr, content_len) -> i32
//! ```

use std::cell::RefCell;
use std::collections::HashMap;

use graph::agent::interactive_architect::InteractiveArchitectAgent;
use graph::agent::new_feature_architect::NewFeatureArchitectAgent;
use graph::agent::new_feature_pm::NewFeatureProductManagerAgent;
use graph::agent::new_feature_se::NewFeatureSoftwareEngineerAgent;
use graph::agent::product_manager::ProductManagerAgent;
use graph::agent::software_architect::SoftwareArchitectAgent;
use graph::filesystem::{FileSystem, FsEntry};
use graph::indexer::{GraphIndexer, IndexResult};
use graph::serializer::GraphSerializer;
use serde::Deserialize;

// ─── Host I/O imports ─────────────────────────────────────────────────────────

unsafe extern "C" {
    /// Ask the host to list directory `dir`.  The host writes a JSON array of
    /// `{"name":"…","is_dir":false}` objects into the response buffer via
    /// `fs_response_reserve` and returns the byte count (0 on error).
    fn host_list(dir_ptr: *const u8, dir_len: u32) -> u32;

    /// Ask the host to read the file at `path`.  The host writes the UTF-8
    /// file content into the response buffer and returns the byte count.
    /// Returns 0 if the file does not exist or cannot be read.
    fn host_read(path_ptr: *const u8, path_len: u32) -> u32;

    /// Ask the host to write `content` to `path` (creating parent dirs as
    /// needed).  Returns 0 on success, -1 on failure.
    fn host_write(
        path_ptr:    *const u8, path_len:    u32,
        content_ptr: *const u8, content_len: u32,
    ) -> i32;
}

// ─── Shared buffers ───────────────────────────────────────────────────────────

thread_local! {
    /// Scratch buffer that the *host* fills when responding to `host_list`,
    /// `host_read`, etc.  The host calls `fs_response_reserve` to get a
    /// pointer, writes bytes, then returns the byte count.
    static RESPONSE_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };

    /// Scratch buffer that *WASM* fills when returning data to the host.
    /// TypeScript calls `wasm_response_ptr()` to retrieve the pointer, then
    /// reads however many bytes were returned by the preceding export call.
    static WASM_RESPONSE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };

    /// Holds the [`IndexResult`] between `wasm_run` and `wasm_set_descriptions`.
    static CURRENT_RESULT: RefCell<Option<IndexResult>> = const { RefCell::new(None) };

    /// Holds the running [`SoftwareArchitectAgent`] between `wasm_architect_new`
    /// and successive `wasm_architect_process_response` calls.
    static CURRENT_ARCHITECT: RefCell<Option<SoftwareArchitectAgent>> = const { RefCell::new(None) };

    /// Holds the running [`ProductManagerAgent`] between `wasm_pm_new`
    /// and successive `wasm_pm_process_response` calls.
    static CURRENT_PM: RefCell<Option<ProductManagerAgent>> = const { RefCell::new(None) };

    /// Holds the running [`InteractiveArchitectAgent`] between `wasm_ia_new`
    /// and successive `wasm_ia_process_response` calls.
    static CURRENT_IA: RefCell<Option<InteractiveArchitectAgent>> = const { RefCell::new(None) };

    /// Holds the running [`NewFeatureProductManagerAgent`] across both phases.
    static CURRENT_NFPM: RefCell<Option<NewFeatureProductManagerAgent>> = const { RefCell::new(None) };

    /// Holds the running [`NewFeatureArchitectAgent`] across both phases.
    static CURRENT_NFA: RefCell<Option<NewFeatureArchitectAgent>> = const { RefCell::new(None) };

    /// Holds the running [`NewFeatureSoftwareEngineerAgent`].
    static CURRENT_NFSE: RefCell<Option<NewFeatureSoftwareEngineerAgent>> = const { RefCell::new(None) };
}

/// Called by the host to allocate (or reuse) a buffer of `size` bytes inside
/// WASM linear memory.  The host then writes its response directly into this
/// region before returning the byte count to Rust.
#[unsafe(no_mangle)]
pub extern "C" fn fs_response_reserve(size: u32) -> *mut u8 {
    RESPONSE_BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.resize(size as usize, 0);
        buf.as_mut_ptr()
    })
}

/// Returns a pointer to the WASM-side response buffer.  TypeScript reads
/// `len` bytes from this address after any `wasm_*` call that returns a
/// non-zero length.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_response_ptr() -> *const u8 {
    WASM_RESPONSE.with(|buf| buf.borrow().as_ptr())
}

fn take_host_response(len: u32) -> Vec<u8> {
    RESPONSE_BUF.with(|buf| buf.borrow()[..len as usize].to_vec())
}

fn read_wasm_string(ptr: *const u8, len: u32) -> String {
    // Safety: the host owns this memory and wrote valid UTF-8 into it.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn write_wasm_response(json: &[u8]) -> u32 {
    WASM_RESPONSE.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        buf.extend_from_slice(json);
    });
    json.len() as u32
}

// ─── HostFileSystem ───────────────────────────────────────────────────────────

struct HostFileSystem;

impl FileSystem for HostFileSystem {
    fn list(&self, dir: &str) -> Vec<FsEntry> {
        let len = unsafe { host_list(dir.as_ptr(), dir.len() as u32) };
        if len == 0 {
            return Vec::new();
        }
        let bytes = take_host_response(len);

        #[derive(Deserialize)]
        struct RawEntry { name: String, is_dir: bool }

        let raw: Vec<RawEntry> = serde_json::from_slice(&bytes).unwrap_or_default();
        raw.into_iter()
            .map(|e| FsEntry { name: e.name, is_dir: e.is_dir })
            .collect()
    }

    fn read(&self, path: &str) -> Option<String> {
        let len = unsafe { host_read(path.as_ptr(), path.len() as u32) };
        if len == 0 {
            return None;
        }
        String::from_utf8(take_host_response(len)).ok()
    }

    fn write(&self, path: &str, content: &str) -> bool {
        let rc = unsafe {
            host_write(
                path.as_ptr(),    path.len()    as u32,
                content.as_ptr(), content.len() as u32,
            )
        };
        rc == 0
    }
}

// ─── Command entry point ──────────────────────────────────────────────────────

/// WASI entry point.  TypeScript calls `wasi.start()` to run this no-op,
/// which lets the WASI runtime initialise linear memory and WASI state.
/// Afterwards TypeScript calls the reactor exports below directly on the
/// live instance — thread-locals (which are plain globals in single-threaded
/// WASM) remain intact across calls.
fn main() {}

// ─── Reactor exports ──────────────────────────────────────────────────────────

/// Run the indexer against `root`.
///
/// TypeScript must:
/// 1. Call `ptr = fs_response_reserve(root_len)`.
/// 2. Write the UTF-8 root path into `memory[ptr..ptr+root_len]`.
/// 3. Call `wasm_run(ptr, root_len, rebuild)`.
///
/// Returns the number of pending description tasks (0 if none).
/// The [`IndexResult`] is stored in `CURRENT_RESULT` for the subsequent calls.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_run(root_ptr: *const u8, root_len: u32, rebuild: i32) -> u32 {
    let root = read_wasm_string(root_ptr, root_len);
    let result = GraphIndexer::new(root, Box::new(HostFileSystem))
        .rebuild(rebuild != 0)
        .run();
    let task_count = result.tasks.len() as u32;
    CURRENT_RESULT.with(|cell| *cell.borrow_mut() = Some(result));
    task_count
}

/// Serialise the pending description tasks to JSON and store them in the WASM
/// response buffer.
///
/// Returns the byte length of the JSON.  TypeScript reads the bytes starting
/// at `wasm_response_ptr()`.
///
/// The JSON schema matches `DescriptionTask[]`:
/// ```json
/// [{ "file": "…", "content": "…", "schema": { "pkg.ClassName": "" } }]
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn wasm_pending_tasks() -> u32 {
    let json = CURRENT_RESULT.with(|cell| {
        let borrow = cell.borrow();
        match borrow.as_ref() {
            Some(r) => serde_json::to_vec(r.pending_tasks()).unwrap_or_default(),
            None    => b"[]".to_vec(),
        }
    });
    write_wasm_response(&json)
}

/// Apply LLM-generated descriptions and persist `graph.yml`.
///
/// TypeScript must:
/// 1. Call `ptr = fs_response_reserve(json_len)`.
/// 2. Write a UTF-8 JSON object `{ "qualified.Name": "description …" }` into
///    `memory[ptr..ptr+json_len]`.
/// 3. Call `wasm_set_descriptions(ptr, json_len)`.
///
/// Returns 0 on success, -1 if there is no pending result (i.e. `wasm_run`
/// was not called first).
#[derive(serde::Deserialize, Default)]
struct EnrichmentPayload {
    #[serde(default)]
    descriptions: HashMap<String, String>,
    #[serde(default)]
    is_test: HashMap<String, bool>,
}

/// Apply LLM-generated descriptions and is_test flags, then persist `graph.yml`.
///
/// TypeScript must write a UTF-8 JSON object with the shape:
/// ```json
/// {
///   "descriptions": { "pkg.ClassName": "…" },
///   "is_test":      { "pkg.ClassName": true }
/// }
/// ```
/// Both fields are optional — omit either to skip that enrichment step.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_set_descriptions(json_ptr: *const u8, json_len: u32) -> i32 {
    let payload: EnrichmentPayload = if json_len == 0 {
        EnrichmentPayload::default()
    } else {
        let json_bytes = unsafe { std::slice::from_raw_parts(json_ptr, json_len as usize) };
        serde_json::from_slice(json_bytes).unwrap_or_default()
    };

    let result = CURRENT_RESULT.with(|cell| cell.borrow_mut().take());
    match result {
        Some(r) => { r.commit(payload.descriptions, payload.is_test); 0 }
        None    => -1,
    }
}

// ─── Architect reactor exports ────────────────────────────────────────────────

/// Load the persisted `graph.yml` from `<root>/.codegraph/graph.yml` and
/// initialise a [`SoftwareArchitectAgent`] ready to explore it.
///
/// TypeScript must:
/// 1. Call `ptr = fs_response_reserve(root_len)`.
/// 2. Write the UTF-8 root path into `memory[ptr..ptr+root_len]`.
/// 3. Call `wasm_architect_new(ptr, root_len)`.
///
/// Returns 0 on success, -1 if the graph file cannot be read or parsed.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_architect_new(root_ptr: *const u8, root_len: u32) -> i32 {
    let root = read_wasm_string(root_ptr, root_len);
    let graph_path = format!("{}/.codegraph/graph.yml", root);

    let yaml = match HostFileSystem.read(&graph_path) {
        Some(y) => y,
        None    => return -1,
    };

    let graph = match GraphSerializer::deserialize(&yaml) {
        Ok(g)  => g,
        Err(_) => return -1,
    };

    let agent = SoftwareArchitectAgent::new(graph, root, Box::new(HostFileSystem));
    CURRENT_ARCHITECT.with(|cell| *cell.borrow_mut() = Some(agent));
    0
}

/// Serialise the current LLM request (messages + tools) into the WASM response
/// buffer.  TypeScript reads the bytes from `wasm_response_ptr()`.
///
/// Returns the byte length of the JSON.  Returns 0 if no architect is active.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_architect_get_request() -> u32 {
    let json = CURRENT_ARCHITECT.with(|cell| {
        cell.borrow().as_ref().map(|a| a.get_request()).unwrap_or_default()
    });
    if json.is_empty() { return 0; }
    write_wasm_response(json.as_bytes())
}

/// Feed the LLM response JSON to the active [`SoftwareArchitectAgent`].
///
/// TypeScript must:
/// 1. Call `ptr = fs_response_reserve(response_len)`.
/// 2. Write the UTF-8 OpenAI response JSON into `memory[ptr..ptr+response_len]`.
/// 3. Call `wasm_architect_process_response(ptr, response_len)`.
///
/// The result is written into the WASM response buffer as a JSON object:
/// ```json
/// { "action": "continue" }
/// { "action": "message", "content": "…markdown…" }
/// { "action": "stop" }
/// { "action": "error",   "content": "…reason…" }
/// ```
///
/// When `action` is `"message"`, the architecture document has already been
/// written to `<root>/.codegraph/architecture.md` by WASM.
///
/// Returns the byte length of the result JSON.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_architect_process_response(ptr: *const u8, len: u32) -> u32 {
    use graph::agent::llm_agent::AgentAction;

    let response = read_wasm_string(ptr, len);

    let action_json = CURRENT_ARCHITECT.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => r#"{"action":"error","content":"no active architect agent"}"#.to_owned(),
            Some(agent) => match agent.process_response(&response) {
                AgentAction::Continue          => r#"{"action":"continue"}"#.to_owned(),
                AgentAction::AskQuestions(_)   => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Stop     => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Error(e) => {
                    let msg = serde_json::to_string(&e).unwrap_or_else(|_| r#""unknown""#.to_owned());
                    format!(r#"{{"action":"error","content":{}}}"#, msg)
                }
                AgentAction::AssistantMessage(doc) => {
                    // Serialise now — we need `doc` for both writing and the response.
                    let json = serde_json::to_string(&doc)
                        .unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"message","content":{}}}"#, json)
                }
            },
        }
    });

    // If the action is "message", also persist the architecture document.
    if action_json.starts_with(r#"{"action":"message""#) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&action_json) {
            if let Some(doc) = v["content"].as_str() {
                // Retrieve root from the architect's first user message — the
                // path is embedded there.  As a fallback we use the graph path
                // written to WASM_RESPONSE (we don't store root separately).
                // Instead, extract it from the active agent's context.
                CURRENT_ARCHITECT.with(|cell| {
                    if let Some(agent) = cell.borrow().as_ref() {
                        if let Some(arch_path) = agent.architecture_path() {
                            HostFileSystem.write(&arch_path, doc);
                        }
                    }
                });
            }
        }
    }

    write_wasm_response(action_json.as_bytes())
}

// ─── Interactive Architect reactor exports ────────────────────────────────────

/// Load `graph.yml` and initialise an [`InteractiveArchitectAgent`] for a
/// one-off architectural question.
///
/// TypeScript must write a JSON payload `{"root":"…","question":"…"}` into the
/// WASM buffer first:
/// 1. `ptr = fs_response_reserve(json_len)`
/// 2. `memory[ptr..] = utf8_json`
/// 3. `wasm_ia_new(ptr, json_len)`
///
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_ia_new(ptr: *const u8, len: u32) -> i32 {
    #[derive(serde::Deserialize)]
    struct Params { root: String, question: String }

    let json = read_wasm_string(ptr, len);
    let params: Params = match serde_json::from_str(&json) {
        Ok(p)  => p,
        Err(_) => return -1,
    };

    let graph_path = format!("{}/.codegraph/graph.yml", params.root);
    let yaml = match HostFileSystem.read(&graph_path) {
        Some(y) => y,
        None    => return -1,
    };
    let graph = match GraphSerializer::deserialize(&yaml) {
        Ok(g)  => g,
        Err(_) => return -1,
    };

    let agent = InteractiveArchitectAgent::new(
        graph,
        params.root,
        params.question,
        Box::new(HostFileSystem),
    );
    CURRENT_IA.with(|cell| *cell.borrow_mut() = Some(agent));
    0
}

/// Returns the byte length of the IA request JSON (0 if no agent active).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_ia_get_request() -> u32 {
    let json = CURRENT_IA.with(|cell| {
        cell.borrow().as_ref().map(|a| a.get_request()).unwrap_or_default()
    });
    if json.is_empty() { return 0; }
    write_wasm_response(json.as_bytes())
}

/// Feed the LLM response to the IA agent.
/// Returns the byte length of the result JSON (same action format as architect).
/// On "message" action the answer is in `content` — TypeScript streams it to chat.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_ia_process_response(ptr: *const u8, len: u32) -> u32 {
    use graph::agent::llm_agent::AgentAction;
    let response = read_wasm_string(ptr, len);
    let action_json = CURRENT_IA.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => r#"{"action":"error","content":"no active interactive architect"}"#.to_owned(),
            Some(agent) => match agent.process_response(&response) {
                AgentAction::Continue            => r#"{"action":"continue"}"#.to_owned(),
                AgentAction::AskQuestions(_)     => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Stop                => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Error(e)            => {
                    let msg = serde_json::to_string(&e).unwrap_or_else(|_| r#""unknown""#.to_owned());
                    format!(r#"{{"action":"error","content":{}}}"#, msg)
                }
                AgentAction::AssistantMessage(answer) => {
                    let json = serde_json::to_string(&answer).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"message","content":{}}}"#, json)
                }
            },
        }
    });
    write_wasm_response(action_json.as_bytes())
}

// ─── Product Manager reactor exports ─────────────────────────────────────────

/// Load the persisted `graph.yml` and initialise a [`ProductManagerAgent`].
///
/// Returns 0 on success, -1 if graph.yml cannot be read or parsed.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_pm_new(root_ptr: *const u8, root_len: u32) -> i32 {
    let root = read_wasm_string(root_ptr, root_len);
    let graph_path = format!("{}/.codegraph/graph.yml", root);
    let yaml = match HostFileSystem.read(&graph_path) {
        Some(y) => y,
        None    => return -1,
    };
    let graph = match GraphSerializer::deserialize(&yaml) {
        Ok(g)  => g,
        Err(_) => return -1,
    };
    let agent = ProductManagerAgent::new(graph, root, Box::new(HostFileSystem));
    CURRENT_PM.with(|cell| *cell.borrow_mut() = Some(agent));
    0
}

/// Returns the byte length of the PM agent request JSON (0 if no agent active).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_pm_get_request() -> u32 {
    let json = CURRENT_PM.with(|cell| {
        cell.borrow().as_ref().map(|a| a.get_request()).unwrap_or_default()
    });
    if json.is_empty() { return 0; }
    write_wasm_response(json.as_bytes())
}

/// Feed the LLM response to the PM agent. Writes specs.md on "message" action.
/// Returns the byte length of the result JSON.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_pm_process_response(ptr: *const u8, len: u32) -> u32 {
    use graph::agent::llm_agent::AgentAction;
    let response = read_wasm_string(ptr, len);
    let action_json = CURRENT_PM.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => r#"{"action":"error","content":"no active product manager agent"}"#.to_owned(),
            Some(agent) => match agent.process_response(&response) {
                AgentAction::Continue          => r#"{"action":"continue"}"#.to_owned(),
                AgentAction::AskQuestions(_)   => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Stop     => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Error(e) => {
                    let msg = serde_json::to_string(&e).unwrap_or_else(|_| r#""unknown""#.to_owned());
                    format!(r#"{{"action":"error","content":{}}}"#, msg)
                }
                AgentAction::AssistantMessage(doc) => {
                    let json = serde_json::to_string(&doc).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"message","content":{}}}"#, json)
                }
            },
        }
    });
    if action_json.starts_with(r#"{"action":"message""#) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&action_json) {
            if let Some(doc) = v["content"].as_str() {
                CURRENT_PM.with(|cell| {
                    if let Some(agent) = cell.borrow().as_ref() {
                        if let Some(path) = agent.specs_path() {
                            HostFileSystem.write(&path, doc);
                        }
                    }
                });
            }
        }
    }
    write_wasm_response(action_json.as_bytes())
}

// ─── New Feature Product Manager reactor exports ──────────────────────────────

/// Initialise a [`NewFeatureProductManagerAgent`].
///
/// TypeScript writes JSON `{"root":"…","feature":"…"}` into the buffer first.
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfpm_new(ptr: *const u8, len: u32) -> i32 {
    #[derive(serde::Deserialize)]
    struct Params { root: String, feature: String }

    let json = read_wasm_string(ptr, len);
    let params: Params = match serde_json::from_str(&json) {
        Ok(p)  => p,
        Err(_) => return -1,
    };

    let graph_path = format!("{}/.codegraph/graph.yml", params.root);
    let yaml = match HostFileSystem.read(&graph_path) {
        Some(y) => y,
        None    => return -1,
    };
    let graph = match GraphSerializer::deserialize(&yaml) {
        Ok(g)  => g,
        Err(_) => return -1,
    };

    let agent = NewFeatureProductManagerAgent::new(
        graph,
        params.root,
        params.feature,
        Box::new(HostFileSystem),
    );
    CURRENT_NFPM.with(|cell| *cell.borrow_mut() = Some(agent));
    0
}

/// Returns the byte length of the NFPM request JSON (0 if no agent active).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfpm_get_request() -> u32 {
    let json = CURRENT_NFPM.with(|cell| {
        cell.borrow().as_ref().map(|a| a.get_request()).unwrap_or_default()
    });
    if json.is_empty() { return 0; }
    write_wasm_response(json.as_bytes())
}

/// Feed the LLM response to the NFPM agent.
///
/// Action JSON format:
///   {"action":"continue"}                         — tool calls executed
///   {"action":"questions","content":"[…]"}        — questions JSON array
///   {"action":"message",  "content":"…markdown…"} — feature spec (no file written here)
///   {"action":"stop"}
///   {"action":"error","content":"…"}
///
/// Note: the spec file is written by TypeScript (it needs to determine the path).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfpm_process_response(ptr: *const u8, len: u32) -> u32 {
    use graph::agent::llm_agent::AgentAction;
    let response = read_wasm_string(ptr, len);
    let action_json = CURRENT_NFPM.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => r#"{"action":"error","content":"no active nfpm agent"}"#.to_owned(),
            Some(agent) => match agent.process_response(&response) {
                AgentAction::Continue          => r#"{"action":"continue"}"#.to_owned(),
                AgentAction::Stop              => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::AskQuestions(args) => {
                    // args is the raw JSON arguments from the ask_questions tool call.
                    let json = serde_json::to_string(&args).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"questions","content":{}}}"#, json)
                }
                AgentAction::Error(e) => {
                    let msg = serde_json::to_string(&e).unwrap_or_else(|_| r#""unknown""#.to_owned());
                    format!(r#"{{"action":"error","content":{}}}"#, msg)
                }
                AgentAction::AssistantMessage(content) => {
                    let json = serde_json::to_string(&content).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"message","content":{}}}"#, json)
                }
            },
        }
    });
    write_wasm_response(action_json.as_bytes())
}

/// Inject the user's answers as a new user message and prepare for phase 2.
///
/// TypeScript writes the answers JSON string into the buffer.
/// The agent appends a user message with the answers so the next
/// `wasm_nfpm_get_request` + LLM call generates the feature spec.
///
/// Returns 0 on success, -1 if no agent is active.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfpm_submit_answers(ptr: *const u8, len: u32) -> i32 {
    let answers = read_wasm_string(ptr, len);
    CURRENT_NFPM.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => -1,
            Some(agent) => { agent.submit_answers(&answers); 0 }
        }
    })
}

// ─── New Feature Architect reactor exports ────────────────────────────────────

/// Initialise a [`NewFeatureArchitectAgent`].
///
/// TypeScript writes JSON `{"root":"…","feature_path":"…"}` into the buffer.
/// `feature_path` is the absolute path to the feature directory
/// (e.g. `/project/.codegraph/features/001-add-python-support`).
///
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfa_new(ptr: *const u8, len: u32) -> i32 {
    #[derive(serde::Deserialize)]
    struct Params { root: String, feature_path: String }

    let json = read_wasm_string(ptr, len);
    let params: Params = match serde_json::from_str(&json) {
        Ok(p)  => p,
        Err(_) => return -1,
    };

    let graph_path = format!("{}/.codegraph/graph.yml", params.root);
    let yaml = match HostFileSystem.read(&graph_path) {
        Some(y) => y,
        None    => return -1,
    };
    let graph = match GraphSerializer::deserialize(&yaml) {
        Ok(g)  => g,
        Err(_) => return -1,
    };

    let agent = NewFeatureArchitectAgent::new(
        graph,
        params.root,
        params.feature_path,
        Box::new(HostFileSystem),
    );
    CURRENT_NFA.with(|cell| *cell.borrow_mut() = Some(agent));
    0
}

/// Returns the byte length of the NFA request JSON (0 if no agent active).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfa_get_request() -> u32 {
    let json = CURRENT_NFA.with(|cell| {
        cell.borrow().as_ref().map(|a| a.get_request()).unwrap_or_default()
    });
    if json.is_empty() { return 0; }
    write_wasm_response(json.as_bytes())
}

/// Feed the LLM response to the NFA agent.
///
/// Actions:
///   {"action":"continue"}
///   {"action":"questions","content":"[…]"}
///   {"action":"message","content":"…markdown…"}  — plan ready, TypeScript writes plan.md
///   {"action":"stop"}
///   {"action":"error","content":"…"}
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfa_process_response(ptr: *const u8, len: u32) -> u32 {
    use graph::agent::llm_agent::AgentAction;
    let response = read_wasm_string(ptr, len);
    let action_json = CURRENT_NFA.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => r#"{"action":"error","content":"no active nfa agent"}"#.to_owned(),
            Some(agent) => match agent.process_response(&response) {
                AgentAction::Continue           => r#"{"action":"continue"}"#.to_owned(),
                AgentAction::Stop               => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::AskQuestions(args) => {
                    let json = serde_json::to_string(&args).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"questions","content":{}}}"#, json)
                }
                AgentAction::Error(e) => {
                    let msg = serde_json::to_string(&e).unwrap_or_else(|_| r#""unknown""#.to_owned());
                    format!(r#"{{"action":"error","content":{}}}"#, msg)
                }
                AgentAction::AssistantMessage(content) => {
                    let json = serde_json::to_string(&content).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"message","content":{}}}"#, json)
                }
            },
        }
    });
    write_wasm_response(action_json.as_bytes())
}

/// Inject the developer's answers and prepare for plan generation.
/// Returns 0 on success, -1 if no agent is active.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfa_submit_answers(ptr: *const u8, len: u32) -> i32 {
    let answers = read_wasm_string(ptr, len);
    CURRENT_NFA.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => -1,
            Some(agent) => { agent.submit_answers(&answers); 0 }
        }
    })
}

// ─── New Feature Software Engineer reactor exports ────────────────────────────

/// Initialise a [`NewFeatureSoftwareEngineerAgent`].
///
/// TypeScript writes JSON `{"root":"…","feature_path":"…"}` into the buffer.
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfse_new(ptr: *const u8, len: u32) -> i32 {
    #[derive(serde::Deserialize)]
    struct Params { root: String, feature_path: String }

    let json = read_wasm_string(ptr, len);
    let params: Params = match serde_json::from_str(&json) {
        Ok(p)  => p,
        Err(_) => return -1,
    };

    let graph_path = format!("{}/.codegraph/graph.yml", params.root);
    let yaml = match HostFileSystem.read(&graph_path) {
        Some(y) => y,
        None    => return -1,
    };
    let graph = match GraphSerializer::deserialize(&yaml) {
        Ok(g)  => g,
        Err(_) => return -1,
    };

    let agent = NewFeatureSoftwareEngineerAgent::new(
        graph,
        params.root,
        params.feature_path,
        Box::new(HostFileSystem),
    );
    CURRENT_NFSE.with(|cell| *cell.borrow_mut() = Some(agent));
    0
}

/// Returns the byte length of the NFSE request JSON (0 if no agent active).
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfse_get_request() -> u32 {
    let json = CURRENT_NFSE.with(|cell| {
        cell.borrow().as_ref().map(|a| a.get_request()).unwrap_or_default()
    });
    if json.is_empty() { return 0; }
    write_wasm_response(json.as_bytes())
}

/// Feed the LLM response to the NFSE agent.
///
/// Actions:
///   {"action":"continue"}
///   {"action":"message","content":"…tasks markdown…"}  — tasks ready, TypeScript writes tasks.md
///   {"action":"stop"}
///   {"action":"error","content":"…"}
#[unsafe(no_mangle)]
pub extern "C" fn wasm_nfse_process_response(ptr: *const u8, len: u32) -> u32 {
    use graph::agent::llm_agent::AgentAction;
    let response = read_wasm_string(ptr, len);
    let action_json = CURRENT_NFSE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        match borrow.as_mut() {
            None        => r#"{"action":"error","content":"no active nfse agent"}"#.to_owned(),
            Some(agent) => match agent.process_response(&response) {
                AgentAction::Continue          => r#"{"action":"continue"}"#.to_owned(),
                AgentAction::AskQuestions(_)   => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Stop     => r#"{"action":"stop"}"#.to_owned(),
                AgentAction::Error(e) => {
                    let msg = serde_json::to_string(&e).unwrap_or_else(|_| r#""unknown""#.to_owned());
                    format!(r#"{{"action":"error","content":{}}}"#, msg)
                }
                AgentAction::AssistantMessage(content) => {
                    let json = serde_json::to_string(&content).unwrap_or_else(|_| r#""""#.to_owned());
                    format!(r#"{{"action":"message","content":{}}}"#, json)
                }
            },
        }
    });
    write_wasm_response(action_json.as_bytes())
}
