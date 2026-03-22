import * as vscode from 'vscode';
import { handleAnalyze } from './analyze';

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
  _context: vscode.ChatContext,
  stream:  vscode.ChatResponseStream,
  token:   vscode.CancellationToken,
): Promise<vscode.ChatResult> {
  switch (request.command) {
    case 'analyze':
      await handleAnalyze(request, stream, token);
      break;

    default:
      stream.markdown(
        'Hi! I\'m **CodeGraph**. Use `/analyze` to index your workspace and ' +
        'generate an architecture document and product specification.\n\n' +
        '```\n@codegraph /analyze\n```',
      );
  }

  return {};
}
