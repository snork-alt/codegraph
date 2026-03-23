import * as path from 'path';
import * as fs from 'fs';
import * as vscode from 'vscode';
import { createInteractiveArchitectSession, InteractiveArchitectSession } from '@codegraph/common-ts';
import { createCopilotLLMClient } from './llm-bridge';

// ─── Session cache ────────────────────────────────────────────────────────────

/**
 * One live WASM session per workspace root.  The session is reused for
 * follow-up questions in the same chat thread (context.history is non-empty)
 * and discarded when a fresh conversation starts.
 */
const activeSessions = new Map<string, InteractiveArchitectSession>();

/**
 * Handle a free-form architectural question directed at @codegraph.
 *
 * Requires `.codegraph/graph.yml` to exist in the workspace root.
 * Streams tool action details into the chat as the agent explores,
 * then streams the final answer.
 *
 * Prior conversation turns from the same @codegraph session are prepended
 * to the question so the agent has full context for follow-up questions.
 */
export async function handleAsk(
  request: vscode.ChatRequest,
  context: vscode.ChatContext,
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

  // Reuse the live WASM session if the user is continuing the same thread,
  // start a fresh one when this is the first message in a new conversation.
  const isFollowUp = context.history.some(
    t => t instanceof vscode.ChatRequestTurn && !t.command,
  );

  let session = activeSessions.get(rootPath);
  if (!session || !isFollowUp) {
    session = await createInteractiveArchitectSession(rootPath);
    activeSessions.set(rootPath, session);
  }

  let answerStarted = false;

  const llm = createCopilotLLMClient(
    request.model,
    token,
    (toolName, actionDetails) => {
      stream.markdown(`- ${actionDetails || toolName}\n`);
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
    await session.ask(question, llm);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    stream.markdown(`**Error:** ${msg}`);
    return;
  }

  if (!answerStarted) {
    stream.markdown('The agent could not produce an answer.');
  }
}

