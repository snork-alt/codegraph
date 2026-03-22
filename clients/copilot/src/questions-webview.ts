import * as vscode from 'vscode';
import type { FeatureQuestion } from '@codegraph/common-ts';

/**
 * Show a WebView panel with the clarification questions.
 * Returns a Promise that resolves with the user's answers when submitted,
 * or rejects if the panel is closed without submitting.
 */
export function showQuestionsPanel(
  feature:   string,
  questions: FeatureQuestion[],
): Promise<Record<string, string>> {
  return new Promise((resolve, reject) => {
    const panel = vscode.window.createWebviewPanel(
      'codegraph.questions',
      'CodeGraph — Feature Clarification',
      vscode.ViewColumn.One,
      { enableScripts: true },
    );

    panel.webview.html = buildHtml(feature, questions);

    panel.webview.onDidReceiveMessage((msg: { type: string; answers?: Record<string, string> }) => {
      if (msg.type === 'submit' && msg.answers) {
        resolve(msg.answers);
        panel.dispose();
      }
    });

    panel.onDidDispose(() => reject(new Error('Questions panel closed without submitting.')));
  });
}

// ─── HTML builder ─────────────────────────────────────────────────────────────

function buildHtml(feature: string, questions: FeatureQuestion[]): string {
  const questionsHtml = questions.map((q, i) => {
    const inputHtml = q.type === 'choice' && q.choices?.length
      ? q.choices.map(c => `
          <label class="choice-label">
            <input type="radio" name="${esc(q.id)}" value="${esc(c)}" required />
            ${esc(c)}
          </label>`).join('\n')
      : `<textarea name="${esc(q.id)}" rows="3" placeholder="Your answer…" required></textarea>`;

    return `
      <div class="question">
        <p class="question-text"><span class="q-num">${i + 1}.</span> ${esc(q.text)}</p>
        <div class="input-group">${inputHtml}</div>
      </div>`;
  }).join('\n');

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Feature Clarification</title>
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

    body {
      font-family: var(--vscode-font-family, sans-serif);
      font-size: var(--vscode-font-size, 13px);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 32px;
      max-width: 720px;
      margin: 0 auto;
    }

    h1 { font-size: 1.3em; margin-bottom: 6px; }
    .subtitle {
      color: var(--vscode-descriptionForeground);
      margin-bottom: 32px;
      font-style: italic;
    }

    .question {
      margin-bottom: 28px;
      padding: 18px 20px;
      border: 1px solid var(--vscode-panel-border, #444);
      border-radius: 6px;
      background: var(--vscode-editorWidget-background, #1e1e1e);
    }

    .question-text {
      font-weight: 600;
      margin-bottom: 12px;
      line-height: 1.5;
    }

    .q-num {
      color: var(--vscode-textLink-foreground, #3794ff);
      margin-right: 4px;
    }

    textarea {
      width: 100%;
      padding: 8px 10px;
      background: var(--vscode-input-background);
      color: var(--vscode-input-foreground);
      border: 1px solid var(--vscode-input-border, #555);
      border-radius: 4px;
      font-family: inherit;
      font-size: inherit;
      resize: vertical;
    }

    textarea:focus {
      outline: 1px solid var(--vscode-focusBorder, #007fd4);
      border-color: var(--vscode-focusBorder, #007fd4);
    }

    .choice-label {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 6px 0;
      cursor: pointer;
    }

    .choice-label input { cursor: pointer; accent-color: var(--vscode-textLink-foreground, #3794ff); }

    .actions { margin-top: 32px; }

    button {
      padding: 8px 22px;
      background: var(--vscode-button-background, #0e639c);
      color: var(--vscode-button-foreground, #fff);
      border: none;
      border-radius: 4px;
      font-size: inherit;
      cursor: pointer;
    }

    button:hover { background: var(--vscode-button-hoverBackground, #1177bb); }
  </style>
</head>
<body>
  <h1>Feature Clarification</h1>
  <p class="subtitle">${esc(feature)}</p>

  <form id="form">
    ${questionsHtml}
    <div class="actions">
      <button type="submit">Submit answers</button>
    </div>
  </form>

  <script>
    const vscode = acquireVsCodeApi();
    document.getElementById('form').addEventListener('submit', (e) => {
      e.preventDefault();
      const data = new FormData(e.target);
      const answers = {};
      for (const [key, value] of data.entries()) {
        answers[key] = value;
      }
      vscode.postMessage({ type: 'submit', answers });
    });
  </script>
</body>
</html>`;
}

function esc(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}
