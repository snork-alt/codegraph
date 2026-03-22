#!/usr/bin/env node
import 'dotenv/config';

import * as path from 'node:path';
import { Command } from 'commander';
import { runIndex, runArchitect, runProductManager } from '@codegraph/common-ts';
import { createOpenAIClient, createArchitectLLMClient } from './llm/openai';

const program = new Command();

program
  .name('codegraph')
  .description('Index source code into a dependency graph')
  .version('0.1.0');

program
  .command('index <path>')
  .description(
    'Recursively scan <path> for supported source files and write ' +
    '<path>/.codegraph/graph.yml',
  )
  .option('-r, --rebuild',      'ignore existing graph.yml and rebuild from scratch')
  .option('-d, --descriptions', 'enrich entity descriptions via the OpenAI API')
  .action(async (targetPath: string, opts: { rebuild?: boolean; descriptions?: boolean }) => {
    const llm = opts.descriptions ? createOpenAIClient() : undefined;
    await runIndex(targetPath, opts.rebuild ?? false, llm);
  });

program
  .command('architect <path>')
  .description(
    'Explore the dependency graph at <path>/.codegraph/graph.yml using an LLM ' +
    'and write an architecture document to <path>/.codegraph/architecture.md. ' +
    'Requires OPENAI_API_KEY to be set.',
  )
  .action(async (targetPath: string) => {
    const absTarget = path.resolve(targetPath);
    const llm = createArchitectLLMClient();
    console.log(`Generating architecture document for ${absTarget} …`);
    await runArchitect(absTarget, llm);
    console.log(`✓ Architecture written to ${absTarget}/.codegraph/architecture.md`);
  });

program
  .command('product-manager <path>')
  .description(
    'Read the architecture document at <path>/.codegraph/architecture.md and the ' +
    'dependency graph, then write a product specification to ' +
    '<path>/.codegraph/specs.md. Run "codegraph architect" first. ' +
    'Requires OPENAI_API_KEY to be set.',
  )
  .action(async (targetPath: string) => {
    const absTarget = path.resolve(targetPath);
    const llm = createArchitectLLMClient();
    console.log(`Generating product specification for ${absTarget} …`);
    await runProductManager(absTarget, llm);
    console.log(`✓ Specs written to ${absTarget}/.codegraph/specs.md`);
  });

program.parseAsync(process.argv).catch((err: unknown) => {
  console.error(err);
  process.exit(1);
});
