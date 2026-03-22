import * as path from 'path';
import * as fs from 'fs';
import * as vscode from 'vscode';
import { runInteractiveArchitect } from '@codegraph/common-ts';
import { createCopilotLLMClient } from './llm-bridge';

/**
 * Handle a free-form architectural question directed at @codegraph.
 *
 * Requires `.codegraph/graph.yml` to exist in the workspace root.
 * Streams tool action details into the chat as the agent explores,
 * then streams the final answer.
 */
export async function handleAsk(
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
    stream.markdown(
      'No dependency graph found. Run `@codegraph /analyze` first to index the workspace.',
    );
    return;
  }

  const question = request.prompt.trim();
  if (!question) {
    stream.markdown('Please ask a question, e.g. `@codegraph How is authentication handled?`');
    return;
  }

  let answerStarted = false;

  const llm = createCopilotLLMClient(
    request.model,
    token,
    (_toolName, actionDetails) => {
      if (actionDetails) { stream.markdown(`- ${actionDetails}\n`); }
    },
    (fragment) => {
      if (!answerStarted) {
        stream.markdown('\n');
        answerStarted = true;
      }
      stream.markdown(fragment);
    },
  );

  try {
    await runInteractiveArchitect(rootPath, question, llm);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    stream.markdown(`**Error:** ${msg}`);
    return;
  }

  if (!answerStarted) {
    stream.markdown('The agent could not produce an answer.');
  }
}
