import * as path from 'path';
import * as fs   from 'fs';
import * as vscode from 'vscode';
import { createNewFeatureArchitectSession } from '@codegraph/common-ts';
import { createCopilotLLMClient } from './llm-bridge';
import { showQuestionsPanel } from './questions-webview';
import { stripPreamble } from './utils';

/**
 * Handle `@codegraph /plan [feature-name]`.
 *
 * Flow:
 *  - No argument: list all features that don't have a plan.md yet.
 *  - With argument: find the matching feature directory, run the architect
 *    agent, optionally ask clarification questions, generate plan.md.
 */
export async function handlePlan(
  request: vscode.ChatRequest,
  _context: vscode.ChatContext,
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
    listPlannableFeatures(stream, featuresRoot);
    return;
  }

  // ── Resolve feature directory ────────────────────────────────────────────
  const featurePath = resolveFeaturePath(featuresRoot, arg);
  if (!featurePath) {
    stream.markdown(
      `Could not find a feature matching **${arg}**.\n\n` +
      'Run `/plan` with no argument to see available features.',
    );
    return;
  }

  const specsFile = path.join(featurePath, 'specs.md');
  if (!fs.existsSync(specsFile)) {
    stream.markdown(`Feature found at \`${path.relative(rootPath, featurePath)}\` but it has no \`specs.md\`. Run \`/specify\` first.`);
    return;
  }

  const planFile = path.join(featurePath, 'plan.md');
  if (fs.existsSync(planFile)) {
    stream.markdown(`A plan already exists for **${path.basename(featurePath)}**. Opening preview…`);
    await vscode.commands.executeCommand('markdown.showPreview', vscode.Uri.file(planFile));
    return;
  }

  stream.markdown(`**Planning feature:** ${path.basename(featurePath)}\n\n`);

  // ── Phase 1: Explore ─────────────────────────────────────────────────────
  let session;
  try {
    session = await createNewFeatureArchitectSession(rootPath, featurePath, request.model.family);
  } catch (err) {
    stream.markdown(`**Error:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  const exploreLLM = createCopilotLLMClient(
    request.model, token,
    (tool, details) => { stream.markdown(`- ${details || tool}\n`); },
  );

  let questions;
  try {
    questions = await session.exploreAndGetQuestions(exploreLLM);
  } catch (err) {
    stream.markdown(`\n**Error during exploration:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  // ── Phase 2: Questions WebView (if needed) ───────────────────────────────
  let answers: Record<string, string> = {};

  if (questions.length > 0) {
    stream.markdown(`\n${questions.length} clarification question(s) — please answer them in the panel that just opened.\n`);

    try {
      answers = await showQuestionsPanel(path.basename(featurePath), questions);
    } catch {
      stream.markdown('**Cancelled** — panel was closed without submitting answers.');
      return;
    }

    stream.markdown('\n**Generating implementation plan…**\n\n');
  } else {
    stream.markdown('\n**Generating implementation plan…**\n\n');
  }

  // ── Phase 3: Generate plan ────────────────────────────────────────────────
  const planLLM = createCopilotLLMClient(
    request.model, token,
    (tool, details) => { stream.markdown(`- ${details || tool}\n`); },
  );

  let plan: string;
  try {
    plan = await session.generatePlan(answers, planLLM);
  } catch (err) {
    stream.markdown(`\n**Error generating plan:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  plan = stripPreamble(plan);

  if (!plan.trim()) {
    stream.markdown('\nThe agent did not produce a plan.');
    return;
  }

  // ── Save plan ─────────────────────────────────────────────────────────────

  try {
    fs.writeFileSync(planFile, plan, 'utf-8');
  } catch (err) {
    stream.markdown(`\n**Error saving plan:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  stream.markdown(`\n\n✓ Plan saved to \`${path.relative(rootPath, planFile)}\``);

  // ── Open preview ──────────────────────────────────────────────────────────
  await vscode.commands.executeCommand('markdown.showPreview', vscode.Uri.file(planFile));
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/**
 * List all features in `featuresRoot` that don't yet have a `plan.md`.
 */
function listPlannableFeatures(
  stream:       vscode.ChatResponseStream,
  featuresRoot: string,
): void {
  if (!fs.existsSync(featuresRoot)) {
    stream.markdown('No features found. Run `/specify` first to create a feature specification.');
    return;
  }

  const unplanned = fs
    .readdirSync(featuresRoot, { withFileTypes: true })
    .filter(e => e.isDirectory() && !fs.existsSync(path.join(featuresRoot, e.name, 'plan.md')))
    .map(e => e.name)
    .sort();

  if (unplanned.length === 0) {
    stream.markdown('All features already have an implementation plan.');
    return;
  }

  stream.markdown('**Features without an implementation plan:**\n\n');
  for (const name of unplanned) {
    stream.markdown(`- \`${name}\`\n`);
  }
  stream.markdown('\nRun `/plan <feature-name>` to generate a plan for one of the above.');
}

/**
 * Find the feature directory whose name contains `arg` (case-insensitive prefix or substring).
 * Returns `undefined` if nothing matches or multiple ambiguous matches exist.
 */
function resolveFeaturePath(featuresRoot: string, arg: string): string | undefined {
  if (!fs.existsSync(featuresRoot)) return undefined;

  const lower = arg.toLowerCase();
  const entries = fs
    .readdirSync(featuresRoot, { withFileTypes: true })
    .filter(e => e.isDirectory())
    .map(e => e.name);

  // Exact match first.
  const exact = entries.find(n => n.toLowerCase() === lower);
  if (exact) return path.join(featuresRoot, exact);

  // Prefix match.
  const prefixMatches = entries.filter(n => n.toLowerCase().startsWith(lower));
  if (prefixMatches.length === 1) return path.join(featuresRoot, prefixMatches[0]);

  // Substring match.
  const subMatches = entries.filter(n => n.toLowerCase().includes(lower));
  if (subMatches.length === 1) return path.join(featuresRoot, subMatches[0]);

  return undefined;
}
