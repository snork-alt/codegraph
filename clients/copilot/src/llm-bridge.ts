/**
 * Adapts the VS Code Language Model API (GitHub Copilot) to the
 * ArchitectLLMClient interface expected by @codegraph/common-ts.
 *
 * The WASM layer communicates in OpenAI request/response JSON.
 * This module translates between that format and the vscode.lm types.
 */

import * as vscode from 'vscode';
import type { ArchitectLLMClient, LLMClient } from '@codegraph/common-ts';

// ─── OpenAI wire types (what WASM sends / expects back) ───────────────────────

interface OAIMessage {
  role:          'system' | 'user' | 'assistant' | 'tool';
  content?:      string | null;
  tool_calls?:   OAIToolCall[];
  tool_call_id?: string;
}

interface OAIToolCall {
  id:       string;
  type:     'function';
  function: { name: string; arguments: string };
}

interface OAITool {
  type:     'function';
  function: { name: string; description: string; parameters: unknown };
}

interface OAIRequest {
  messages: OAIMessage[];
  tools?:   OAITool[];
}

// ─── Message conversion ────────────────────────────────────────────────────────

function toVsCodeMessages(messages: OAIMessage[]): vscode.LanguageModelChatMessage[] {
  const result: vscode.LanguageModelChatMessage[] = [];

  for (let i = 0; i < messages.length; i++) {
    const msg = messages[i];
    switch (msg.role) {
      case 'system':
        // vscode.lm has no system role — prepend as a labelled User message.
        result.push(vscode.LanguageModelChatMessage.User(
          `[System instructions]\n${msg.content ?? ''}`,
        ));
        break;

      case 'user':
        result.push(vscode.LanguageModelChatMessage.User(msg.content ?? ''));
        break;

      case 'assistant': {
        const parts: Array<vscode.LanguageModelTextPart | vscode.LanguageModelToolCallPart> = [];

        if (msg.content) {
          parts.push(new vscode.LanguageModelTextPart(msg.content));
        }

        for (const tc of msg.tool_calls ?? []) {
          let input: unknown;
          try { input = JSON.parse(tc.function.arguments); } catch { input = {}; }
          parts.push(new vscode.LanguageModelToolCallPart(
            tc.id,
            tc.function.name,
            input as Record<string, unknown>,
          ));
        }

        result.push(vscode.LanguageModelChatMessage.Assistant(parts));
        break;
      }

      case 'tool': {
        // Anthropic requires ALL tool results for one assistant turn to be in
        // a SINGLE user message.  Collect every consecutive tool message here
        // so they become one User message with multiple ToolResultParts rather
        // than separate messages (which would trigger a 400 "unexpected
        // tool_use_id" error because only the first result would immediately
        // follow the assistant tool_use block).
        const parts: vscode.LanguageModelToolResultPart[] = [];
        while (i < messages.length && messages[i].role === 'tool') {
          const toolMsg = messages[i];
          parts.push(new vscode.LanguageModelToolResultPart(
            toolMsg.tool_call_id ?? '',
            [new vscode.LanguageModelTextPart(toolMsg.content ?? '')],
          ));
          i++;
        }
        i--; // outer loop will increment past the last consumed index
        result.push(vscode.LanguageModelChatMessage.User(parts));
        break;
      }
    }
  }

  return result;
}

// ─── Tool conversion ──────────────────────────────────────────────────────────

function toVsCodeTools(tools: OAITool[]): vscode.LanguageModelChatTool[] {
  return tools.map(t => {
    // Strip additionalProperties — some models (e.g. Claude via Copilot) refuse
    // to emit structured tool calls when this constraint is present.
    const { additionalProperties: _, ...schema } = t.function.parameters as Record<string, unknown>;
    return {
      name:        t.function.name,
      description: t.function.description,
      inputSchema: schema,
    };
  });
}

// ─── Response collection ──────────────────────────────────────────────────────

async function collectResponse(
  response:    vscode.LanguageModelChatResponse,
  onToolCall?: OnToolCall,
  onText?:     (fragment: string) => void,
): Promise<{ text: string; toolCalls: OAIToolCall[] }> {
  let text = '';
  const toolCalls: OAIToolCall[] = [];

  for await (const part of response.stream) {
    if (part instanceof vscode.LanguageModelTextPart) {
      text += part.value;
      onText?.(part.value);
    } else if (part instanceof vscode.LanguageModelToolCallPart) {
      const tc: OAIToolCall = {
        id:   part.callId,
        type: 'function',
        function: {
          name:      part.name,
          arguments: JSON.stringify(part.input),
        },
      };
      toolCalls.push(tc);

      // Fire immediately so action details appear in the chat as each tool
      // call arrives, not batched at the end of the full response.
      if (onToolCall) {
        let details = '';
        try {
          const args = part.input as Record<string, unknown>;
          details = typeof args['__actionDetails__'] === 'string' ? args['__actionDetails__'] : '';
        } catch { /* ignore */ }
        onToolCall(part.name, details);
      }
    }
  }

  // Fallback: some models (e.g. Claude via Copilot) emit tool calls as text
  // in a <function=name>\n<parameter=key\nvalue\n</parameter>\n</function> format
  // instead of structured LanguageModelToolCallPart events.
  if (toolCalls.length === 0 && text.includes('<function=')) {
    const parsed = parseTextToolCalls(text);
    if (parsed.toolCalls.length > 0) {
      // Fire onToolCall for text-parsed tool calls too.
      if (onToolCall) {
        for (const tc of parsed.toolCalls) {
          let details = '';
          try {
            const args = JSON.parse(tc.function.arguments) as Record<string, unknown>;
            details = typeof args['__actionDetails__'] === 'string' ? args['__actionDetails__'] : '';
          } catch { /* ignore */ }
          onToolCall(tc.function.name, details);
        }
      }
      return { text: parsed.remainingText, toolCalls: parsed.toolCalls };
    }
  }

  return { text, toolCalls };
}

/**
 * Parse text-based tool calls emitted by models that don't use structured
 * tool call events. Handles the format:
 *   <function=tool_name>
 *   <parameter=param_name>value</parameter>
 *   </function>
 */
function parseTextToolCalls(text: string): { remainingText: string; toolCalls: OAIToolCall[] } {
  const toolCalls: OAIToolCall[] = [];

  // Match <function=name> ... </function> blocks (including </tool_call> variants).
  const fnPattern = /<function=(\w+)>([\s\S]*?)<\/function>/g;
  let match: RegExpExecArray | null;
  let idCounter = 0;

  while ((match = fnPattern.exec(text)) !== null) {
    const name = match[1];
    const body = match[2];
    const args: Record<string, string> = {};

    // Parameters may appear as:
    //   <parameter=key>value</parameter>
    // or (Claude variant without closing > on opening tag):
    //   <parameter=key\nvalue\n</parameter>
    const paramPattern = /<parameter=([^>\n]+?)(?:>|\n)([\s\S]*?)<\/parameter>/g;
    let paramMatch: RegExpExecArray | null;

    while ((paramMatch = paramPattern.exec(body)) !== null) {
      args[paramMatch[1].trim()] = paramMatch[2].trim();
    }

    toolCalls.push({
      id:   `tc_${idCounter++}`,
      type: 'function',
      function: { name, arguments: JSON.stringify(args) },
    });
  }

  // Strip all tool call blocks (and any surrounding </tool_call> wrappers) from the text.
  const remainingText = text
    .replace(/<function=\w+>[\s\S]*?<\/function>/g, '')
    .replace(/<\/tool_call>/g, '')
    .trim();

  return { remainingText, toolCalls };
}

// ─── Public factory ───────────────────────────────────────────────────────────

/**
 * Fired once per tool call the LLM makes, with the tool name and the
 * `__actionDetails__` the model supplied (or an empty string if absent).
 */
export type OnToolCall = (toolName: string, actionDetails: string) => void;

/**
 * Fired for each text fragment streamed by the LLM in the final answer turn.
 * Not called for preamble text that precedes tool calls.
 */
export type OnText = (fragment: string) => void;

/**
 * Create an {@link ArchitectLLMClient} backed by the GitHub Copilot LLM.
 *
 * @param model       The VS Code language model chosen by the user.
 * @param cancelToken Cancellation token forwarded from the chat request.
 * @param onToolCall  Optional callback fired for each tool call with its name
 *                    and `__actionDetails__` — use this to drive progress UI.
 * @param onText      Optional callback fired for each text fragment in the LLM
 *                    response — use this to stream the final answer in real time.
 */
export function createCopilotLLMClient(
  model:       vscode.LanguageModelChat,
  cancelToken: vscode.CancellationToken,
  onToolCall?: OnToolCall,
  onText?:     OnText,
): ArchitectLLMClient {
  return async (requestJson: string): Promise<string> => {
    const req: OAIRequest = JSON.parse(requestJson);
    const messages = toVsCodeMessages(req.messages);
    const tools    = toVsCodeTools(req.tools ?? []);

    const options: vscode.LanguageModelChatRequestOptions = tools.length > 0
      ? { tools }
      : {};

    let response: vscode.LanguageModelChatResponse;
    try {
      response = await model.sendRequest(messages, options, cancelToken);
    } catch (err) {
      if (err instanceof vscode.LanguageModelError) {
        throw new Error(`Copilot LLM error [${err.code}]: ${err.message}`);
      }
      throw err;
    }

    const { text, toolCalls } = await collectResponse(response, onToolCall, onText);

    // Return an OpenAI-format completion that the WASM agent can parse.
    return JSON.stringify({
      choices: [{
        message: {
          role:       'assistant',
          content:    text || null,
          ...(toolCalls.length > 0 ? { tool_calls: toolCalls } : {}),
        },
        finish_reason: toolCalls.length > 0 ? 'tool_calls' : 'stop',
      }],
    });
  };
}

/**
 * Create a simple {@link LLMClient} for description enrichment backed by
 * the GitHub Copilot LLM.
 *
 * The enrichment pipeline sends plain-text prompts and expects plain-text
 * JSON back — no tool calls involved.
 */
export function createCopilotEnrichmentClient(
  model:       vscode.LanguageModelChat,
  cancelToken: vscode.CancellationToken,
): LLMClient {
  return async (prompt: string): Promise<string> => {
    const messages = [vscode.LanguageModelChatMessage.User(prompt)];

    let response: vscode.LanguageModelChatResponse;
    try {
      response = await model.sendRequest(messages, {}, cancelToken);
    } catch (err) {
      if (err instanceof vscode.LanguageModelError) {
        throw new Error(`Copilot LLM error [${err.code}]: ${err.message}`);
      }
      throw err;
    }

    let text = '';
    for await (const part of response.stream) {
      if (part instanceof vscode.LanguageModelTextPart) {
        text += part.value;
      }
    }
    return text;
  };
}
