/**
 * OpenAI-compatible LLM client for codegraph description enrichment.
 *
 * Configuration via environment variables / `.env`:
 *
 *   OPENAI_API_KEY    — required; your API key
 *   OPENAI_BASE_URL   — optional; override base URL (Azure, Ollama, LM Studio…)
 *   OPENAI_MODEL      — optional; model name (default: gpt-4o-mini)
 *   OPENAI_MAX_TOKENS — optional; max output tokens per request (default: 4096)
 *   OPENAI_TIMEOUT_MS — optional; request timeout in ms (default: 120000)
 */

import OpenAI from 'openai';
import type { LLMClient, ArchitectLLMClient } from '@codegraph/common-ts';

const DEFAULT_MODEL      = 'gpt-4o-mini';
const DEFAULT_MAX_TOKENS = 4096;
const DEFAULT_TIMEOUT_MS = 120_000;

/**
 * Build a `LLMClient` backed by the OpenAI SDK.
 * Throws at call-time if `OPENAI_API_KEY` is absent.
 */
export function createOpenAIClient(): LLMClient {
  return async (prompt: string): Promise<string> => {
    const apiKey    = process.env['OPENAI_API_KEY'];
    const baseURL   = process.env['OPENAI_BASE_URL'];
    const model     = process.env['OPENAI_MODEL']      ?? DEFAULT_MODEL;
    const maxTokens = parseInt(process.env['OPENAI_MAX_TOKENS'] ?? String(DEFAULT_MAX_TOKENS), 10);
    const timeoutMs = parseInt(process.env['OPENAI_TIMEOUT_MS'] ?? String(DEFAULT_TIMEOUT_MS), 10);

    if (!apiKey) {
      throw new Error(
        'OPENAI_API_KEY is not set.\n' +
        'Add it to a .env file in the project root or export it in your shell.',
      );
    }

    const client = new OpenAI({
      apiKey,
      timeout: timeoutMs,
      ...(baseURL ? { baseURL } : {}),
    });

    const response = await client.chat.completions.create({
      model,
      max_tokens: maxTokens,
      messages: [{ role: 'user', content: prompt }],
      response_format: { type: 'json_object' },
    });

    const finishReason = response.choices[0]?.finish_reason;
    if (finishReason === 'length') {
      throw new Error(
        `Response truncated (finish_reason=length). ` +
        `Increase OPENAI_MAX_TOKENS (currently ${maxTokens}) or reduce OPENAI_BATCH_SIZE.`,
      );
    }

    return response.choices[0]?.message?.content ?? '{}';
  };
}

/**
 * Build an `ArchitectLLMClient` backed by the OpenAI SDK.
 *
 * This client accepts a full OpenAI chat request JSON (messages + tools)
 * produced by the WASM architect agent and returns the raw completion JSON.
 *
 * Configuration via the same env vars as `createOpenAIClient`, with one
 * addition:
 *   OPENAI_ARCHITECT_MODEL — model to use for architecture generation
 *                            (default: OPENAI_MODEL ?? gpt-4o)
 */
export function createArchitectLLMClient(): ArchitectLLMClient {
  return async (requestJson: string): Promise<string> => {
    const apiKey    = process.env['OPENAI_API_KEY'];
    const baseURL   = process.env['OPENAI_BASE_URL'];
    const model     = process.env['OPENAI_ARCHITECT_MODEL']
                   ?? process.env['OPENAI_MODEL']
                   ?? 'gpt-4o';
    const maxTokens = parseInt(
      process.env['OPENAI_ARCHITECT_MAX_TOKENS'] ?? process.env['OPENAI_MAX_TOKENS'] ?? '16384',
      10,
    );
    const timeoutMs = parseInt(process.env['OPENAI_TIMEOUT_MS'] ?? String(DEFAULT_TIMEOUT_MS), 10);

    if (!apiKey) {
      throw new Error(
        'OPENAI_API_KEY is not set.\n' +
        'Add it to a .env file in the project root or export it in your shell.',
      );
    }

    const client = new OpenAI({
      apiKey,
      timeout: timeoutMs,
      ...(baseURL ? { baseURL } : {}),
    });

    // Parse the request JSON produced by the WASM agent.
    const req = JSON.parse(requestJson) as {
      messages: OpenAI.Chat.ChatCompletionMessageParam[];
      tools?:   OpenAI.Chat.ChatCompletionTool[];
    };

    const response = await client.chat.completions.create({
      model,
      max_tokens:  maxTokens,
      messages:    req.messages,
      tools:       req.tools,
      tool_choice: req.tools && req.tools.length > 0 ? 'auto' : undefined,
    });

    return JSON.stringify(response);
  };
}
