import * as vscode from 'vscode';
import { handleAnalyze } from './analyze';
import { handleAsk } from './ask';
import { handleSpecify } from './specify';
import { handlePlan } from './plan';
import { handleTasks } from './tasks';

const PARTICIPANT_ID = 'codegraph.codegraph';

export function activate(context: vscode.ExtensionContext): void {
  const participant = vscode.chat.createChatParticipant(
    PARTICIPANT_ID,
    handler,
  );

  participant.iconPath = vscode.Uri.joinPath(context.extensionUri, 'icon.png');

  context.subscriptions.push(participant);
}

export function deactivate(): void { /* nothing to clean up */ }

// ─── Chat handler ─────────────────────────────────────────────────────────────

async function handler(
  request: vscode.ChatRequest,
  context: vscode.ChatContext,
  stream:  vscode.ChatResponseStream,
  token:   vscode.CancellationToken,
): Promise<vscode.ChatResult> {
  switch (request.command) {
    case 'analyze':
      await handleAnalyze(request, context, stream, token);
      break;

    case 'specify':
      await handleSpecify(request, context, stream, token);
      break;

    case 'plan':
      await handlePlan(request, context, stream, token);
      break;

    case 'tasks':
      await handleTasks(request, context, stream, token);
      break;

    default:
      await handleAsk(request, context, stream, token);
  }

  return {};
}
