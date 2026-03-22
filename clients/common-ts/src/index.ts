export type { DescriptionTask, IndexSession, ArchitectLLMClient } from './bridge';
export { indexGraph, runArchitect, runProductManager } from './bridge';
export type { LLMClient } from './enrichment';
export {
  buildPrompt,
  extractPartialSchema,
  enrichFile,
  splitIntoBatches,
  withConcurrency,
  enrichDescriptions,
  runIndex,
} from './enrichment';
