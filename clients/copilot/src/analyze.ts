import * as vscode from 'vscode';
import { indexGraph, enrichDescriptions, runArchitect, runProductManager } from '@codegraph/common-ts';
import { createCopilotLLMClient, createCopilotEnrichmentClient } from './llm-bridge';

/**
 * Handle the `@codegraph /analyze` command.
 *
 * Steps:
 *  1. Index + enrich the workspace  → .codegraph/graph.yml
 *  2. Run architect agent           → .codegraph/architecture.md
 *  3. Run PM agent                  → .codegraph/specs.md
 *
 * All progress is streamed directly into the chat history.
 */
export async function handleAnalyze(
  request: vscode.ChatRequest,
  stream:  vscode.ChatResponseStream,
  token:   vscode.CancellationToken,
): Promise<void> {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders?.length) {
    stream.markdown('**No workspace folder is open.** Open a project folder first.');
    return;
  }

  const rootPath = folders[0].uri.fsPath;

  try {
    // ── Step 1: Index ──────────────────────────────────────────────────────────
    stream.markdown('**Step 1 — Indexing & enriching source files…**\n\n');
    const session      = await indexGraph(rootPath, /* rebuild */ false);
    const enrichClient = createCopilotEnrichmentClient(request.model, token);

    if (session.tasks.length > 0) {
      const entityCount   = session.tasks.reduce((n, t) => n + Object.keys(t.schema).length, 0);
      const concurrency   = vscode.workspace.getConfiguration('codegraph').get<number>('enrichmentConcurrency', 5);
      process.env['OPENAI_CONCURRENCY'] = String(concurrency);
      stream.markdown(`Enriching descriptions for ${entityCount} entities across ${session.tasks.length} file(s) (concurrency: ${concurrency})…\n\n`);
      const descriptions = await enrichDescriptions(session.tasks, enrichClient, (file, index, total) => {
        const name = file.split('/').pop() ?? file;
        stream.markdown(`- [${index}/${total}] \`${name}\`\n`);
      });
      session.applyDescriptions(descriptions);
    } else {
      session.applyDescriptions({});
    }
    stream.markdown('✓ `graph.yml` saved.\n\n');

    if (token.isCancellationRequested) { return; }

    // ── Step 2: Architect ──────────────────────────────────────────────────────
    stream.markdown('**Step 2 — Architect agent**\n\n');

    const architectLLM = createCopilotLLMClient(
      request.model,
      token,
      (_toolName, actionDetails) => {
        if (actionDetails) { stream.markdown(`- ${actionDetails}\n`); }
      },
    );

    await runArchitect(rootPath, architectLLM);
    stream.markdown('\n✓ `architecture.md` saved.\n\n');

    const architectUri = vscode.Uri.file(`${rootPath}/.codegraph/architecture.md`);
    await vscode.commands.executeCommand('markdown.showPreview', architectUri);

    if (token.isCancellationRequested) { return; }

    // ── Step 3: Product Manager ────────────────────────────────────────────────
    stream.markdown('**Step 3 — Product manager agent**\n\n');

    const pmLLM = createCopilotLLMClient(
      request.model,
      token,
      (_toolName, actionDetails) => {
        if (actionDetails) { stream.markdown(`- ${actionDetails}\n`); }
      },
    );

    await runProductManager(rootPath, pmLLM);
    stream.markdown('\n✓ `specs.md` saved.\n\n');

    const specsUri = vscode.Uri.file(`${rootPath}/.codegraph/specs.md`);
    await vscode.commands.executeCommand('markdown.showPreview', specsUri);

  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    stream.markdown(`\n**Analysis failed:** ${msg}`);
    return;
  }

  stream.markdown(
    '\n**Analysis complete!** Files written to `.codegraph/`:\n\n' +
    '- **`graph.yml`** — full dependency graph\n' +
    '- **`architecture.md`** — software architecture document\n' +
    '- **`specs.md`** — product specification\n',
  );
}
