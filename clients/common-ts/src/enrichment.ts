import * as fs   from 'node:fs';
import * as path from 'node:path';

import { indexGraph, DescriptionTask } from './bridge';

// ─── LLM client type ──────────────────────────────────────────────────────────

/**
 * A function that sends a prompt to an LLM and returns the raw text response.
 * Implementations are provided by the caller.
 */
export type LLMClient = (prompt: string) => Promise<string>;

// ─── Prompt builder ───────────────────────────────────────────────────────────

/**
 * Build a prompt that asks an LLM to fill in descriptions for the entities
 * in `task.schema`.  Only the per-entity source snippets are included — not
 * the full file — so the prompt stays as short as possible.
 */
export function buildPrompt(task: DescriptionTask): string {
  const entityBlocks = Object.entries(task.snippets)
    .map(([qname, snippet]) => [
      `### ${qname}`,
      '```',
      snippet,
      '```',
    ].join('\n'))
    .join('\n\n');

  const schemaJson   = JSON.stringify(task.schema, null, 2);
  // Only ask about is_test for nodes not already confirmed (null entries).
  const unknownTests = Object.fromEntries(
    Object.entries(task.is_test_schema ?? {}).filter(([, v]) => v === null)
  );
  const isTestJson   = JSON.stringify(unknownTests, null, 2);
  const hasTestWork  = Object.keys(unknownTests).length > 0;

  return [
    `You are a code documentation assistant.`,
    ``,
    `Below are source code snippets from \`${task.file}\`.`,
    `Each snippet shows exactly one code entity.`,
    ``,
    entityBlocks,
    ``,
    `Return a single JSON object with exactly two top-level keys:`,
    ``,
    `1. "descriptions": map each qualified name to a concise one-sentence`,
    `   description of what that entity does.`,
    ``,
    `2. "is_test": map each qualified name to true if the entity is part of a`,
    `   test suite (test class, test method, test helper, mock, fixture, etc.),`,
    `   or false if it is production code.`,
    ``,
    `IMPORTANT: Copy the property names EXACTLY as shown. Do not modify them.`,
    ``,
    `Required keys for "descriptions" (fill every value):`,
    schemaJson,
    ``,
    ...(hasTestWork ? [
      `Required keys for "is_test" (fill every null with true or false):`,
      isTestJson,
      ``,
    ] : [
      `"is_test": {} (no unknown entries — all already determined statically)`,
      ``,
    ]),
    `Rules:`,
    `- Output only valid JSON — no markdown fences, no extra keys, no comments.`,
    `- Every "descriptions" value must be a non-empty string.`,
    `- Every "is_test" value must be a boolean (true or false).`,
  ].join('\n');
}

// ─── Schema extractor ─────────────────────────────────────────────────────────

/**
 * Extract descriptions and is_test flags from a parsed LLM response.
 *
 * The LLM returns `{ descriptions: { ... }, is_test: { ... } }`.
 * For backwards compatibility, a flat object (old format) is treated as
 * containing only descriptions.
 *
 * Returns `{ filled, missing, isTest }`:
 * - `filled`  — keys with a non-empty description string
 * - `missing` — keys from `schema` absent or empty in the response
 * - `isTest`  — keys whose is_test value was returned as a boolean
 *
 * Returns `null` if `parsed` is not a plain object.
 */
export function extractPartialSchema(
  parsed: unknown,
  schema: Record<string, string>,
): { filled: Record<string, string>; missing: string[]; isTest: Record<string, boolean> } | null {
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return null;
  }
  const obj = parsed as Record<string, unknown>;

  // Support both the new two-key format and the old flat format.
  const descObj: Record<string, unknown> =
    (typeof obj['descriptions'] === 'object' && obj['descriptions'] !== null)
      ? obj['descriptions'] as Record<string, unknown>
      : obj;
  const testObj: Record<string, unknown> =
    (typeof obj['is_test'] === 'object' && obj['is_test'] !== null)
      ? obj['is_test'] as Record<string, unknown>
      : {};

  const filled:  Record<string, string>  = {};
  const missing: string[]                = [];
  const isTest:  Record<string, boolean> = {};

  for (const key of Object.keys(schema)) {
    const val = descObj[key];
    if (typeof val === 'string' && val.trim() !== '') {
      filled[key] = val.trim();
    } else {
      missing.push(key);
    }
  }

  for (const [key, val] of Object.entries(testObj)) {
    if (typeof val === 'boolean') {
      isTest[key] = val;
    }
  }

  return { filled, missing, isTest };
}

// ─── Per-file enrichment ──────────────────────────────────────────────────────

const MAX_RETRIES = 3;

export interface EnrichFileResult {
  descriptions: Record<string, string>;
  isTest:       Record<string, boolean>;
}

/**
 * Ask the LLM for descriptions and is_test flags for all entities in `task`.
 *
 * Accepts partial results on each attempt and retries only missing description
 * keys. is_test values confirmed statically are passed through unchanged.
 */
export async function enrichFile(
  task:  DescriptionTask,
  llm:   LLMClient,
  index: number,
  total: number,
): Promise<EnrichFileResult> {
  const entityCount = Object.keys(task.schema).length;
  console.log(`  [${index}/${total}] ${task.file} — ${entityCount} entities`);

  const descriptions: Record<string, string>  = {};
  const isTest:       Record<string, boolean>  = {};
  let remaining = { ...task.schema };

  // Seed is_test with values already confirmed by static heuristics.
  for (const [key, val] of Object.entries(task.is_test_schema ?? {})) {
    if (val !== null) isTest[key] = val;
  }

  for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
    if (Object.keys(remaining).length === 0) break;

    const partialTask: DescriptionTask = {
      file:           task.file,
      snippets:       Object.fromEntries(Object.keys(remaining).map(k => [k, task.snippets[k] ?? ''])),
      schema:         remaining,
      is_test_schema: Object.fromEntries(Object.keys(remaining).map(k => [k, task.is_test_schema?.[k] ?? null])),
    };

    try {
      const raw    = await llm(buildPrompt(partialTask));
      const parsed = JSON.parse(raw.trim());
      const result = extractPartialSchema(parsed, remaining);

      if (result === null) {
        console.warn(
          `  [${index}/${total}] ${task.file} — invalid response` +
          ` (attempt ${attempt + 1}/${MAX_RETRIES}), retrying ${Object.keys(remaining).length} entities…`,
        );
        continue;
      }

      Object.assign(descriptions, result.filled);
      Object.assign(isTest, result.isTest);
      remaining = Object.fromEntries(result.missing.map(k => [k, '']));

      if (result.missing.length > 0 && attempt < MAX_RETRIES - 1) {
        console.warn(
          `  [${index}/${total}] ${task.file} — ${Object.keys(result.filled).length} filled,` +
          ` ${result.missing.length} missing (attempt ${attempt + 1}/${MAX_RETRIES}), retrying missing…`,
        );
      }
    } catch (err) {
      console.warn(
        `  [${index}/${total}] ${task.file} — LLM error` +
        ` (attempt ${attempt + 1}/${MAX_RETRIES}): ${err}, retrying…`,
      );
    }
  }

  const filledCount = Object.keys(descriptions).length;
  if (Object.keys(remaining).length > 0) {
    console.warn(
      `  [${index}/${total}] ${task.file} — done (${filledCount}/${entityCount} described,` +
      ` ${Object.keys(remaining).length} could not be filled after ${MAX_RETRIES} attempts)`,
    );
  } else {
    console.log(`  [${index}/${total}] ${task.file} — done (${filledCount}/${entityCount})`);
  }
  return { descriptions, isTest };
}

// ─── Batching ─────────────────────────────────────────────────────────────────

const DEFAULT_BATCH_SIZE = 20;

/** Split a large task into sub-tasks of at most `batchSize` entities each. */
export function splitIntoBatches(task: DescriptionTask, batchSize: number): DescriptionTask[] {
  const keys = Object.keys(task.schema);
  if (keys.length <= batchSize) return [task];

  const batches: DescriptionTask[] = [];
  for (let i = 0; i < keys.length; i += batchSize) {
    const slice    = keys.slice(i, i + batchSize);
    const snippets = Object.fromEntries(slice.map(k => [k, task.snippets[k] ?? '']));
    const schema   = Object.fromEntries(slice.map(k => [k, '']));
    const is_test_schema = Object.fromEntries(slice.map(k => [k, task.is_test_schema?.[k] ?? null]));
    batches.push({ file: task.file, snippets, schema, is_test_schema });
  }
  return batches;
}

// ─── Concurrency pool ─────────────────────────────────────────────────────────

const DEFAULT_CONCURRENCY = 5;

/** Run `fns` with at most `limit` in-flight at a time. */
export async function withConcurrency<T>(fns: (() => Promise<T>)[], limit: number): Promise<T[]> {
  const results: T[] = new Array(fns.length);
  let next = 0;

  async function worker(): Promise<void> {
    while (next < fns.length) {
      const i = next++;
      results[i] = await fns[i]();
    }
  }

  await Promise.all(Array.from({ length: Math.min(limit, fns.length) }, worker));
  return results;
}

// ─── Parallel enrichment ──────────────────────────────────────────────────────

/**
 * Enrich all `tasks` in parallel (up to `OPENAI_CONCURRENCY` at a time),
 * splitting large files into batches of at most `OPENAI_BATCH_SIZE` entities.
 */
export async function enrichDescriptions(
  tasks:    DescriptionTask[],
  llm:      LLMClient,
  onFile?:  (file: string, index: number, total: number) => void,
): Promise<Record<string, string>> {
  const limit     = parseInt(process.env['OPENAI_CONCURRENCY'] ?? String(DEFAULT_CONCURRENCY), 10);
  const batchSize = parseInt(process.env['OPENAI_BATCH_SIZE']  ?? String(DEFAULT_BATCH_SIZE),  10);

  const batched    = tasks.flatMap(t => splitIntoBatches(t, batchSize));
  const splitCount = tasks.filter(t => Object.keys(t.schema).length > batchSize).length;
  if (splitCount > 0) {
    console.log(`  split ${splitCount} large file(s) into ${batched.length} total requests`);
  }
  console.log(`  concurrency: ${limit} parallel requests, batch size: ${batchSize} entities`);

  const fns     = batched.map((t, i) => () => {
    onFile?.(t.file, i + 1, batched.length);
    return enrichFile(t, llm, i + 1, batched.length);
  });
  const results = await withConcurrency(fns, limit);
  const merged: Record<string, string> = {};
  for (const result of results) {
    Object.assign(merged, result);
  }
  return merged;
}

// ─── Public entry point ───────────────────────────────────────────────────────

/**
 * Index `targetPath`, optionally enrich entity descriptions via `llm`, and
 * write the graph to `<targetPath>/.codegraph/graph.yml`.
 */
export async function runIndex(
  targetPath: string,
  rebuild = false,
  llm?: LLMClient,
): Promise<void> {
  const absTarget = path.resolve(targetPath);

  if (!fs.existsSync(absTarget)) {
    console.error(`error: path does not exist: ${absTarget}`);
    process.exit(1);
  }

  if (!fs.statSync(absTarget).isDirectory()) {
    console.error(`error: not a directory: ${absTarget}`);
    process.exit(1);
  }

  console.log(`Indexing ${absTarget} ${rebuild ? '(full rebuild)' : '(incremental)'} …`);

  const session = await indexGraph(absTarget, rebuild);

  if (session.tasks.length > 0 && llm) {
    const fileCount = session.tasks.length;
    const allKeys   = new Set(session.tasks.flatMap(t => Object.keys(t.schema)));
    const model     = process.env['OPENAI_MODEL'] ?? 'gpt-4o-mini';
    console.log(`Enriching ${allKeys.size} entities across ${fileCount} file(s) using ${model} …`);

    const descriptions = await enrichDescriptions(session.tasks, llm);
    const described    = Object.values(descriptions).filter(v => v.trim() !== '').length;

    const missing = [...allKeys].filter(k => !descriptions[k] || descriptions[k].trim() === '');
    if (missing.length > 0) {
      console.warn(`  ${missing.length} entities could not be described:`);
      for (const qname of missing.slice(0, 20)) console.warn(`    - ${qname}`);
      if (missing.length > 20) console.warn(`    … and ${missing.length - 20} more`);
    }

    session.applyDescriptions(descriptions);
    console.log(`  ${described}/${allKeys.size} entities described`);
  } else {
    if (session.tasks.length > 0 && !llm) {
      console.log(`Skipping description enrichment (--descriptions not set)`);
    }
    session.applyDescriptions({});
  }

  console.log(`✓ Graph written to ${absTarget}/.codegraph/graph.yml`);
}
