import * as vscode from 'vscode';

/**
 * Show a WebView panel that renders `tasks.md` and offers an "Implement it"
 * button. When the button is clicked the panel sends a message to the
 * extension, which opens the Copilot chat pre-filled with an implementation
 * request.
 *
 * @param tasksContent  Raw markdown content of tasks.md.
 * @param tasksPath     Absolute path to tasks.md (used in the chat query).
 * @param rootPath      Absolute workspace root (used to compute a relative path).
 */
export function showTasksPanel(
  tasksContent: string,
  tasksPath:    string,
  rootPath:     string,
): void {
  const panel = vscode.window.createWebviewPanel(
    'codegraphTasks',
    'Feature Tasks',
    vscode.ViewColumn.One,
    { enableScripts: true },
  );

  panel.webview.html = buildHtml(tasksContent);

  panel.webview.onDidReceiveMessage(async (msg: { type: string }) => {
    if (msg.type !== 'implement') return;

    panel.dispose();

    const relPath = tasksPath.startsWith(rootPath)
      ? tasksPath.slice(rootPath.length + 1)
      : tasksPath;

    await vscode.commands.executeCommand('workbench.action.chat.open', {
      query:
        `@workspace Please implement the feature tasks defined in \`${relPath}\`. ` +
        `Read the file first, then implement each task in order, modifying the necessary source files. ` +
        `After completing all tasks, run the verification steps described at the end of the file.`,
    });
  });
}

// ─── HTML builder ─────────────────────────────────────────────────────────────

function buildHtml(markdown: string): string {
  const rendered = renderMarkdown(markdown);

  return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Feature Tasks</title>
  <style>
    :root {
      --bg:        var(--vscode-editor-background, #1e1e1e);
      --fg:        var(--vscode-editor-foreground, #d4d4d4);
      --accent:    var(--vscode-button-background, #0e639c);
      --accent-fg: var(--vscode-button-foreground, #ffffff);
      --border:    var(--vscode-panel-border, #444);
      --code-bg:   var(--vscode-textCodeBlock-background, #2d2d2d);
      --heading:   var(--vscode-textLink-foreground, #569cd6);
    }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--bg);
      color: var(--fg);
      font-family: var(--vscode-font-family, system-ui, sans-serif);
      font-size: 14px;
      line-height: 1.6;
      padding: 0;
    }

    /* ── Sticky header ── */
    .header {
      position: sticky;
      top: 0;
      z-index: 10;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      padding: 12px 24px;
      background: var(--bg);
      border-bottom: 1px solid var(--border);
    }
    .header h2 {
      font-size: 15px;
      font-weight: 600;
      color: var(--heading);
      white-space: nowrap;
    }
    .implement-btn {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 8px 18px;
      background: var(--accent);
      color: var(--accent-fg);
      border: none;
      border-radius: 4px;
      font-size: 13px;
      font-weight: 600;
      cursor: pointer;
      white-space: nowrap;
    }
    .implement-btn:hover { opacity: 0.85; }
    .implement-btn svg { flex-shrink: 0; }

    /* ── Content ── */
    .content {
      max-width: 860px;
      margin: 0 auto;
      padding: 24px 32px 48px;
    }
    h1 { font-size: 22px; color: var(--heading); margin: 0 0 20px; }
    h2 { font-size: 17px; color: var(--heading); margin: 28px 0 10px; padding-bottom: 4px; border-bottom: 1px solid var(--border); }
    h3 { font-size: 15px; color: var(--heading); margin: 18px 0 8px; }
    p  { margin: 8px 0; }
    ul, ol { margin: 8px 0 8px 20px; }
    li { margin: 3px 0; }
    strong { color: var(--vscode-symbolIcon-fieldForeground, #9cdcfe); }
    code {
      font-family: var(--vscode-editor-font-family, 'Courier New', monospace);
      font-size: 12px;
      background: var(--code-bg);
      padding: 1px 5px;
      border-radius: 3px;
    }
    pre {
      background: var(--code-bg);
      border: 1px solid var(--border);
      border-radius: 4px;
      padding: 12px 16px;
      overflow-x: auto;
      margin: 10px 0;
    }
    pre code { background: none; padding: 0; font-size: 13px; }
    hr { border: none; border-top: 1px solid var(--border); margin: 20px 0; }
    .task-block {
      border-left: 3px solid var(--accent);
      padding-left: 14px;
      margin: 16px 0;
    }
  </style>
</head>
<body>
  <div class="header">
    <h2>Implementation Tasks</h2>
    <button class="implement-btn" onclick="implement()">
      <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
        <path d="M4 2l8 6-8 6V2z"/>
      </svg>
      Implement with Copilot
    </button>
  </div>
  <div class="content">
    ${rendered}
  </div>
  <script>
    const vscode = acquireVsCodeApi();
    function implement() {
      vscode.postMessage({ type: 'implement' });
    }
  </script>
</body>
</html>`;
}

// ─── Minimal markdown renderer ────────────────────────────────────────────────

function renderMarkdown(md: string): string {
  const lines  = md.split('\n');
  const out: string[] = [];
  let inCode   = false;
  let codeLang = '';
  let codeLines: string[] = [];
  let inTask   = false;

  const flush = () => {
    if (inTask) { out.push('</div>'); inTask = false; }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    // ── Fenced code blocks ───────────────────────────────────────────────────
    if (!inCode && line.startsWith('```')) {
      codeLang   = line.slice(3).trim();
      inCode     = true;
      codeLines  = [];
      continue;
    }
    if (inCode) {
      if (line.startsWith('```')) {
        const escaped = codeLines.map(escHtml).join('\n');
        out.push(`<pre><code class="lang-${codeLang}">${escaped}</code></pre>`);
        inCode = false;
      } else {
        codeLines.push(line);
      }
      continue;
    }

    // ── Headings ─────────────────────────────────────────────────────────────
    const h1 = line.match(/^#\s+(.+)/);
    const h2 = line.match(/^##\s+(.+)/);
    const h3 = line.match(/^###\s+(.+)/);

    if (h1) { flush(); out.push(`<h1>${inlineHtml(h1[1])}</h1>`); continue; }
    if (h2) {
      flush();
      // Wrap tasks in a styled block
      if (/^Task\s+\d+/i.test(h2[1])) {
        out.push('<div class="task-block">');
        inTask = true;
      }
      out.push(`<h2>${inlineHtml(h2[1])}</h2>`);
      continue;
    }
    if (h3) { out.push(`<h3>${inlineHtml(h3[1])}</h3>`); continue; }

    // ── HR ───────────────────────────────────────────────────────────────────
    if (/^---+$/.test(line.trim())) { flush(); out.push('<hr>'); continue; }

    // ── Lists ────────────────────────────────────────────────────────────────
    if (/^[-*]\s/.test(line)) {
      out.push(`<ul><li>${inlineHtml(line.slice(2))}</li></ul>`); continue;
    }
    if (/^\d+\.\s/.test(line)) {
      out.push(`<ol><li>${inlineHtml(line.replace(/^\d+\.\s/, ''))}</li></ol>`); continue;
    }

    // ── Empty line ───────────────────────────────────────────────────────────
    if (line.trim() === '') { out.push('<p></p>'); continue; }

    // ── Paragraph ────────────────────────────────────────────────────────────
    out.push(`<p>${inlineHtml(line)}</p>`);
  }

  flush();

  // Collapse adjacent list elements into single lists.
  return out.join('\n')
    .replace(/<\/ul>\n<ul>/g, '')
    .replace(/<\/ol>\n<ol>/g, '');
}

function escHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function inlineHtml(s: string): string {
  return escHtml(s)
    // **bold**
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    // `code`
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    // _italic_
    .replace(/\b_(.+?)_\b/g, '<em>$1</em>');
}
