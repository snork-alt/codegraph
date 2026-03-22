import * as path from 'path';
import * as fs   from 'fs';
import * as vscode from 'vscode';
import { createNewFeaturePMSession } from '@codegraph/common-ts';
import { createCopilotLLMClient } from './llm-bridge';
import { showQuestionsPanel } from './questions-webview';

/**
 * Handle `@codegraph /specify <feature description>`.
 *
 * Flow:
 *  1. Explore codebase → questions (or skip to spec if clear enough)
 *  2. Show questions in a WebView panel
 *  3. User submits answers
 *  4. Generate feature spec markdown
 *  5. Save to .codegraph/features/<NNN>-<slug>/specs.md
 *  6. Open markdown preview
 */
export async function handleSpecify(
  request: vscode.ChatRequest,
  stream:  vscode.ChatResponseStream,
  token:   vscode.CancellationToken,
): Promise<void> {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders?.length) {
    stream.markdown('**No workspace folder is open.** Open a project folder first.');
    return;
  }

  const rootPath  = folders[0].uri.fsPath;
  const graphFile = path.join(rootPath, '.codegraph', 'graph.yml');

  if (!fs.existsSync(graphFile)) {
    stream.markdown('No dependency graph found. Run `@codegraph /analyze` first.');
    return;
  }

  const feature = request.prompt.trim();
  if (!feature) {
    stream.markdown('Please describe the feature, e.g. `@codegraph /specify Add user authentication`');
    return;
  }

  // ── Phase 1: Explore ────────────────────────────────────────────────────────
  stream.markdown(`**Analysing feature:** ${feature}\n\n`);

  let session;
  try {
    session = await createNewFeaturePMSession(rootPath, feature);
  } catch (err) {
    stream.markdown(`**Error:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  const exploreLLM = createCopilotLLMClient(
    request.model, token,
    (_tool, details) => { if (details) stream.markdown(`- ${details}\n`); },
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
      answers = await showQuestionsPanel(feature, questions);
    } catch {
      // Panel closed without submitting.
      stream.markdown('**Cancelled** — panel was closed without submitting answers.');
      return;
    }

    stream.markdown('\n**Generating feature specification…**\n\n');
  } else {
    stream.markdown('\n**Generating feature specification…**\n\n');
  }

  // ── Phase 3: Generate spec ────────────────────────────────────────────────
  const specLLM = createCopilotLLMClient(
    request.model, token,
    (_tool, details) => { if (details) stream.markdown(`- ${details}\n`); },
    (fragment) => stream.markdown(fragment),
  );

  let spec: string;
  try {
    spec = await session.generateSpec(answers, specLLM);
  } catch (err) {
    stream.markdown(`\n**Error generating spec:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  if (!spec.trim()) {
    stream.markdown('\nThe agent did not produce a specification.');
    return;
  }

  // ── Save spec ─────────────────────────────────────────────────────────────
  const featureDir  = resolveFeatureDir(rootPath, spec);
  const specFile    = path.join(featureDir, 'specs.md');

  try {
    fs.mkdirSync(featureDir, { recursive: true });
    fs.writeFileSync(specFile, spec, 'utf-8');
  } catch (err) {
    stream.markdown(`\n**Error saving spec:** ${err instanceof Error ? err.message : String(err)}`);
    return;
  }

  stream.markdown(`\n\n✓ Spec saved to \`${path.relative(rootPath, specFile)}\``);

  // ── Open preview ──────────────────────────────────────────────────────────
  await vscode.commands.executeCommand('markdown.showPreview', vscode.Uri.file(specFile));
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/**
 * Determine the output directory for the feature spec.
 * Scans `.codegraph/features/` for existing `NNN-*` dirs to get the next number.
 * Derives the slug from the first `# Title` line of the spec.
 */
function resolveFeatureDir(rootPath: string, spec: string): string {
  const featuresRoot = path.join(rootPath, '.codegraph', 'features');

  // Next sequential number.
  let nextNum = 1;
  if (fs.existsSync(featuresRoot)) {
    const entries = fs.readdirSync(featuresRoot);
    const numbers = entries
      .map(e => parseInt(e.slice(0, 3), 10))
      .filter(n => !isNaN(n));
    if (numbers.length > 0) nextNum = Math.max(...numbers) + 1;
  }

  const num  = String(nextNum).padStart(3, '0');
  const slug = titleToSlug(spec);
  return path.join(featuresRoot, `${num}-${slug}`);
}

/** Extract the first `# Title` line and convert it to a filesystem slug. */
function titleToSlug(spec: string): string {
  const match = spec.match(/^#\s+(.+)$/m);
  if (!match) return 'feature';
  return match[1]
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 50);
}
