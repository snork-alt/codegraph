import * as path from 'path';
import * as fs   from 'fs';
import * as vscode from 'vscode';
import { runNewFeatureSE } from '@codegraph/common-ts';
import { createCopilotLLMClient } from './llm-bridge';
import { showTasksPanel } from './tasks-webview';
import { stripPreamble } from './utils';

/**
 * Handle `@codegraph /tasks [feature]`.
 *
 * Flow:
 *  - No argument: list features that have plan.md but no tasks.md.
 *  - With argument: find the feature directory, run the SE agent (single-phase),
 *    save tasks.md, show the preview WebView with "Implement it" button.
 */
export async function handleTasks(
  request: vscode.ChatRequest,
  stream:  vscode.ChatResponseStream,
  token:   vscode.CancellationToken,
): Promise<void> {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders?.length) {
    stream.markdown('**No workspace folder is open.** Open a project folder first.');
    return;
  }

  const rootPath     = folders[0].uri.fsPath;
  const featuresRoot = path.join(rootPath, '.codegraph', 'features');
  const graphFile    = path.join(rootPath, '.codegraph', 'graph.yml');

  if (!fs.existsSync(graphFile)) {
    stream.markdown('No dependency graph found. Run `@codegraph /analyze` first.');
    return;
  }

  const arg = request.prompt.trim();

  if (!arg) {
    listImplementableFeatures(stream, featuresRoot);
    return;
  }

  // ── Resolve feature directory ────────────────────────────────────────────
  const featurePath = resolveFeaturePath(featuresRoot, arg);
  if (!featurePath) {
    stream.markdown(
      `Could not find a feature matching **${arg}**.\n\n` +
      'Run `/tasks` with no argument to see available features.',
    );
    return;
  }

  const specsFile = path.join(featurePath, 'specs.md');
  const planFile  = path.join(featurePath, 'plan.md');

  if (!fs.existsSync(specsFile)) {
    stream.markdown(`Feature found but has no \`specs.md\`. Run \`/specify\` first.`);
    return;
  }
  if (!fs.existsSync(planFile)) {
    stream.markdown(`Feature found but has no \`plan.md\`. Run \`/plan\` first.`);
    return;
  }

  stream.markdown(`**Generating implementation tasks for:** ${path.basename(featurePath)}\n\n`);

  // ── Run SE agent ─────────────────────────────────────────────────────────
  const llm = createCopilotLLMClient(
    request.model, token,
    (_tool, details) => { if (details) stream.markdown(`- ${details}\n`); },
  );

  let tasks: string;
  try {
    tasks = await runNewFeatureSE(rootPath, featurePath, llm);
  } catch (err) {
    stream.markdown(`\n**Error:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  tasks = stripPreamble(tasks);

  if (!tasks.trim()) {
    stream.markdown('\nThe agent did not produce a task list.');
    return;
  }

  // ── Save tasks.md ─────────────────────────────────────────────────────────
  const tasksFile = path.join(featurePath, 'tasks.md');

  try {
    fs.writeFileSync(tasksFile, tasks, 'utf-8');
  } catch (err) {
    stream.markdown(`\n**Error saving tasks:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  stream.markdown(`\n\n✓ Tasks saved to \`${path.relative(rootPath, tasksFile)}\``);
  stream.markdown('\n\nOpening task preview…');

  // ── Show preview WebView with "Implement it" button ───────────────────────
  showTasksPanel(tasks, tasksFile, rootPath);
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/**
 * List features that have a plan.md but no tasks.md yet.
 */
function listImplementableFeatures(
  stream:       vscode.ChatResponseStream,
  featuresRoot: string,
): void {
  if (!fs.existsSync(featuresRoot)) {
    stream.markdown('No features found. Run `/specify` and `/plan` first.');
    return;
  }

  const ready = fs
    .readdirSync(featuresRoot, { withFileTypes: true })
    .filter(e =>
      e.isDirectory() &&
      fs.existsSync(path.join(featuresRoot, e.name, 'plan.md')) &&
      !fs.existsSync(path.join(featuresRoot, e.name, 'tasks.md')),
    )
    .map(e => e.name)
    .sort();

  if (ready.length === 0) {
    const hasPlan = fs
      .readdirSync(featuresRoot, { withFileTypes: true })
      .some(e => e.isDirectory() && fs.existsSync(path.join(featuresRoot, e.name, 'plan.md')));

    if (!hasPlan) {
      stream.markdown('No features with a `plan.md` found. Run `/plan` first.');
    } else {
      stream.markdown('All features with a plan already have a `tasks.md`.');
    }
    return;
  }

  stream.markdown('**Features ready for task generation** (have `plan.md`, no `tasks.md`):\n\n');
  for (const name of ready) {
    stream.markdown(`- \`${name}\`\n`);
  }
  stream.markdown('\nRun `/tasks <feature-name>` to generate implementation tasks.');
}

/**
 * Find the feature directory whose name matches `arg` (exact → prefix → substring).
 */
function resolveFeaturePath(featuresRoot: string, arg: string): string | undefined {
  if (!fs.existsSync(featuresRoot)) return undefined;

  const lower   = arg.toLowerCase();
  const entries = fs
    .readdirSync(featuresRoot, { withFileTypes: true })
    .filter(e => e.isDirectory())
    .map(e => e.name);

  const exact = entries.find(n => n.toLowerCase() === lower);
  if (exact) return path.join(featuresRoot, exact);

  const prefixMatches = entries.filter(n => n.toLowerCase().startsWith(lower));
  if (prefixMatches.length === 1) return path.join(featuresRoot, prefixMatches[0]);

  const subMatches = entries.filter(n => n.toLowerCase().includes(lower));
  if (subMatches.length === 1) return path.join(featuresRoot, subMatches[0]);

  return undefined;
}
