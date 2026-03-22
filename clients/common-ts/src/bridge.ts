/**
 * Low-level bridge between Node.js and the codegraph WASM binary.
 *
 * The binary is a WASI preview1 reactor. TypeScript calls `wasi.start()` to
 * run the no-op main (which lets WASI initialise linear memory), then drives
 * the indexing pipeline by calling exported functions directly:
 *
 *   1. ptr = fs_response_reserve(root_len)
 *      memory[ptr..] = root_utf8
 *      n   = wasm_run(ptr, root_len, rebuild)
 *
 *   2. len = wasm_pending_tasks()
 *      ptr = wasm_response_ptr()
 *      tasks = JSON.parse(memory[ptr..ptr+len])
 *
 *   3. (caller runs LLM enrichment here)
 *
 *   4. ptr = fs_response_reserve(desc_len)
 *      memory[ptr..] = descriptions_json
 *      wasm_set_descriptions(ptr, desc_len)  → writes graph.yml
 *
 * Three host callbacks are provided via the `"env"` import object:
 *
 *   host_list  (dirPtr, dirLen)                           → u32
 *   host_read  (pathPtr, pathLen)                         → u32
 *   host_write (pathPtr, pathLen, contentPtr, contentLen) → i32
 */

import { WASI }  from 'node:wasi';
import * as fs   from 'node:fs';
import * as path from 'node:path';

// Default WASM path: clients/common-wasm/codegraph.wasm (relative to this
// compiled file which lives at clients/common-ts/dist/).
const DEFAULT_WASM_PATH = path.resolve(
  __dirname,
  '..', '..', // common-ts/dist → clients/
  'common-wasm', 'codegraph.wasm',
);

// ─── Types ────────────────────────────────────────────────────────────────────

/**
 * One file's worth of LLM description work.
 *
 * `snippets` maps each qualified name to its extracted source lines.
 * `schema` has the same keys with empty-string values; the caller fills them
 * and passes the completed map to `applyDescriptions`.
 */
export interface DescriptionTask {
  file:     string;
  /** qualified_name → extracted source lines for that node */
  snippets: Record<string, string>;
  /** qualified_name → "" — LLM must fill every value */
  schema:   Record<string, string>;
}

/**
 * The result of an `indexGraph` call.
 *
 * `tasks` lists files that were added or changed and need LLM descriptions.
 * Call `applyDescriptions(descriptions)` to persist `graph.yml`.
 * Pass an empty object to skip description enrichment.
 */
export interface IndexSession {
  tasks: DescriptionTask[];
  applyDescriptions: (descriptions: Record<string, string>) => void;
}

// ─── Memory helpers ───────────────────────────────────────────────────────────

function readString(mem: WebAssembly.Memory, ptr: number, len: number): string {
  return new TextDecoder().decode(new Uint8Array(mem.buffer, ptr, len));
}

function writeToWasm(
  mem:     WebAssembly.Memory,
  exports: Record<string, unknown>,
  text:    string,
): { ptr: number; len: number } {
  const bytes   = new TextEncoder().encode(text);
  const reserve = exports['fs_response_reserve'] as (size: number) => number;
  const ptr     = reserve(bytes.length);
  new Uint8Array(mem.buffer, ptr, bytes.length).set(bytes);
  return { ptr, len: bytes.length };
}

// ─── WASM instance helper ─────────────────────────────────────────────────────

interface WasmInstance {
  memory:  WebAssembly.Memory;
  exports: Record<string, unknown>;
}

async function createWasmInstance(wasmPath: string): Promise<WasmInstance> {
  if (!fs.existsSync(wasmPath)) {
    throw new Error(
      `WASM binary not found at: ${wasmPath}\n` +
      `Build it with: cd graph && cargo build --release --target wasm32-wasip1\n` +
      `Then copy to:  clients/common-wasm/codegraph.wasm`,
    );
  }

  const wasi = new WASI({
    version:      'preview1',
    args:         ['codegraph'],
    env:          process.env as Record<string, string>,
    returnOnExit: true,
  });

  let memory!:  WebAssembly.Memory;
  let exports!: Record<string, unknown>;

  const importObject = {
    wasi_snapshot_preview1: wasi.wasiImport,
    env: {

      host_list: (dirPtr: number, dirLen: number): number => {
        const dir = readString(memory, dirPtr, dirLen);
        let entries: Array<{ name: string; is_dir: boolean }>;
        try {
          entries = fs.readdirSync(dir, { withFileTypes: true }).map(e => ({
            name:   e.name,
            is_dir: e.isDirectory(),
          }));
        } catch {
          return 0;
        }
        const { len } = writeToWasm(memory, exports, JSON.stringify(entries));
        return len;
      },

      host_read: (pathPtr: number, pathLen: number): number => {
        const filePath = readString(memory, pathPtr, pathLen);
        let content: string;
        try {
          content = fs.readFileSync(filePath, 'utf-8');
        } catch {
          return 0;
        }
        const { len } = writeToWasm(memory, exports, content);
        return len;
      },

      host_write: (
        pathPtr:    number, pathLen:    number,
        contentPtr: number, contentLen: number,
      ): number => {
        const filePath = readString(memory, pathPtr,    pathLen);
        const content  = readString(memory, contentPtr, contentLen);
        try {
          fs.mkdirSync(path.dirname(filePath), { recursive: true });
          fs.writeFileSync(filePath, content, 'utf-8');
          return 0;
        } catch {
          return -1;
        }
      },

    },
  };

  const wasmBuffer = fs.readFileSync(wasmPath);
  const wasmModule = await WebAssembly.compile(wasmBuffer);
  const instance   = await WebAssembly.instantiate(wasmModule, importObject);

  memory  = instance.exports['memory'] as WebAssembly.Memory;
  exports = instance.exports as Record<string, unknown>;

  // Run the no-op main() so WASI initialises linear memory and state.
  wasi.start(instance);

  return { memory, exports };
}

// ─── Public API ───────────────────────────────────────────────────────────────

/**
 * Initialise the WASM reactor and run the indexing pass against `rootPath`.
 *
 * @param rootPath  Absolute path to the project root to index.
 * @param rebuild   When true, ignore any existing graph.yml and rebuild.
 * @param wasmPath  Optional override for the WASM binary location.
 *                  Defaults to `CODEGRAPH_WASM` env var, then
 *                  `clients/common-wasm/codegraph.wasm`.
 */
export async function indexGraph(
  rootPath: string,
  rebuild   = false,
  wasmPath?: string,
): Promise<IndexSession> {
  const resolvedWasm = wasmPath ?? process.env['CODEGRAPH_WASM'] ?? DEFAULT_WASM_PATH;
  const { memory, exports } = await createWasmInstance(resolvedWasm);

  // ── 1. Run the indexer ────────────────────────────────────────────────────
  const { ptr: rootPtr, len: rootLen } = writeToWasm(memory, exports, rootPath);
  const wasmRun = exports['wasm_run'] as (ptr: number, len: number, rebuild: number) => number;
  wasmRun(rootPtr, rootLen, rebuild ? 1 : 0);

  // ── 2. Read pending tasks ─────────────────────────────────────────────────
  const wasmPendingTasks = exports['wasm_pending_tasks']  as () => number;
  const wasmResponsePtr  = exports['wasm_response_ptr']   as () => number;

  const tasksLen  = wasmPendingTasks();
  const tasksPtr  = wasmResponsePtr();
  const tasks: DescriptionTask[] = tasksLen > 0
    ? JSON.parse(readString(memory, tasksPtr, tasksLen))
    : [];

  // ── 3. Return session ─────────────────────────────────────────────────────
  const wasmSetDescriptions = exports['wasm_set_descriptions'] as (ptr: number, len: number) => number;

  function applyDescriptions(descriptions: Record<string, string>): void {
    const { ptr, len } = writeToWasm(memory, exports, JSON.stringify(descriptions));
    wasmSetDescriptions(ptr, len);
  }

  return { tasks, applyDescriptions };
}

// ─── Architecture generation ──────────────────────────────────────────────────

/** Architect action returned by each WASM step. */
interface ArchitectAction {
  action:   'continue' | 'message' | 'stop' | 'error';
  content?: string;
}

/**
 * A function that sends an OpenAI-format chat request JSON to an LLM and
 * returns the raw OpenAI chat completion JSON response.
 */
export type ArchitectLLMClient = (requestJson: string) => Promise<string>;

/**
 * Load the persisted `graph.yml` from `<rootPath>/.codegraph/graph.yml`,
 * create a {@link ProductManagerAgent} inside WASM, and drive it to
 * completion by calling `llm` in a loop.
 *
 * Reads `architecture.md` as its first step, then uses the graph tools to
 * understand user flows and features. Writes the result to
 * `<rootPath>/.codegraph/specs.md`.
 *
 * @param rootPath  Absolute path to the project root.
 * @param llm       LLM callback (same type as for `runArchitect`).
 * @param wasmPath  Optional override for the WASM binary location.
 */
export async function runProductManager(
  rootPath: string,
  llm:      ArchitectLLMClient,
  wasmPath?: string,
): Promise<void> {
  const resolvedWasm = wasmPath ?? process.env['CODEGRAPH_WASM'] ?? DEFAULT_WASM_PATH;
  const { memory, exports } = await createWasmInstance(resolvedWasm);

  const wasmResponsePtr  = exports['wasm_response_ptr']      as () => number;
  const wasmPmNew        = exports['wasm_pm_new']            as (ptr: number, len: number) => number;
  const wasmPmGetReq     = exports['wasm_pm_get_request']    as () => number;
  const wasmPmProcess    = exports['wasm_pm_process_response'] as (ptr: number, len: number) => number;

  const { ptr: rootPtr, len: rootLen } = writeToWasm(memory, exports, rootPath);
  const initResult = wasmPmNew(rootPtr, rootLen);
  if (initResult !== 0) {
    throw new Error(
      `Failed to initialise ProductManagerAgent. ` +
      `Make sure '${rootPath}/.codegraph/graph.yml' exists (run 'codegraph index' first).`,
    );
  }

  let turn = 0;
  for (;;) {
    turn++;

    const reqLen = wasmPmGetReq();
    if (reqLen === 0) {
      throw new Error('Product manager agent returned an empty request — this is a bug.');
    }
    const reqPtr  = wasmResponsePtr();
    const reqJson = readString(memory, reqPtr, reqLen);

    console.log(`  [product-manager] turn ${turn}: calling LLM …`);

    const responseJson = await llm(reqJson);

    const { ptr: respPtr, len: respLen } = writeToWasm(memory, exports, responseJson);
    const actionLen = wasmPmProcess(respPtr, respLen);
    const actionPtr = wasmResponsePtr();
    const action: ArchitectAction = JSON.parse(readString(memory, actionPtr, actionLen));

    switch (action.action) {
      case 'continue':
        console.log(`  [product-manager] turn ${turn}: tool calls executed, continuing …`);
        continue;
      case 'message':
        console.log(`  [product-manager] done — specs.md written to ${rootPath}/.codegraph/`);
        return;
      case 'stop':
        console.warn(`  [product-manager] agent stopped without producing a document.`);
        return;
      case 'error':
        throw new Error(`Product manager agent error: ${action.content ?? 'unknown'}`);
    }
  }
}

/**
 * Load the persisted `graph.yml` from `<rootPath>/.codegraph/graph.yml`,
 * create a {@link SoftwareArchitectAgent} inside WASM, and drive it to
 * completion by calling `llm` in a loop.
 *
 * When the agent finishes, WASM writes the architecture document to
 * `<rootPath>/.codegraph/architecture.md`.
 *
 * @param rootPath  Absolute path to the project root (must contain `.codegraph/graph.yml`).
 * @param llm       Callback that calls an LLM with a raw request JSON string and
 *                  returns the raw completion JSON (OpenAI format).
 * @param wasmPath  Optional override for the WASM binary location.
 */
export async function runArchitect(
  rootPath: string,
  llm:      ArchitectLLMClient,
  wasmPath?: string,
): Promise<void> {
  const resolvedWasm = wasmPath ?? process.env['CODEGRAPH_WASM'] ?? DEFAULT_WASM_PATH;
  const { memory, exports } = await createWasmInstance(resolvedWasm);

  const wasmResponsePtr      = exports['wasm_response_ptr']              as () => number;
  const wasmArchitectNew     = exports['wasm_architect_new']             as (ptr: number, len: number) => number;
  const wasmArchitectGetReq  = exports['wasm_architect_get_request']     as () => number;
  const wasmArchitectProcess = exports['wasm_architect_process_response'] as (ptr: number, len: number) => number;

  // ── 1. Initialise the architect agent ─────────────────────────────────────
  const { ptr: rootPtr, len: rootLen } = writeToWasm(memory, exports, rootPath);
  const initResult = wasmArchitectNew(rootPtr, rootLen);
  if (initResult !== 0) {
    throw new Error(
      `Failed to initialise SoftwareArchitectAgent. ` +
      `Make sure '${rootPath}/.codegraph/graph.yml' exists (run 'codegraph index' first).`,
    );
  }

  // ── 2. Agent loop ─────────────────────────────────────────────────────────
  let turn = 0;
  for (;;) {
    turn++;

    // Get the next LLM request from WASM.
    const reqLen = wasmArchitectGetReq();
    if (reqLen === 0) {
      throw new Error('Architect agent returned an empty request — this is a bug.');
    }
    const reqPtr  = wasmResponsePtr();
    const reqJson = readString(memory, reqPtr, reqLen);

    console.log(`  [architect] turn ${turn}: calling LLM …`);

    // Call the LLM.
    const responseJson = await llm(reqJson);

    // Feed the response back to WASM.
    const { ptr: respPtr, len: respLen } = writeToWasm(memory, exports, responseJson);
    const actionLen = wasmArchitectProcess(respPtr, respLen);
    const actionPtr = wasmResponsePtr();
    const action: ArchitectAction = JSON.parse(readString(memory, actionPtr, actionLen));

    switch (action.action) {
      case 'continue':
        console.log(`  [architect] turn ${turn}: tool calls executed, continuing …`);
        continue;

      case 'message':
        console.log(`  [architect] done — architecture.md written to ${rootPath}/.codegraph/`);
        return;

      case 'stop':
        console.warn(`  [architect] agent stopped without producing a document.`);
        return;

      case 'error':
        throw new Error(`Architect agent error: ${action.content ?? 'unknown'}`);
    }
  }
}
