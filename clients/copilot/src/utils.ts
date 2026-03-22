/**
 * Strip any introductory prose that appears before the first markdown heading.
 * Models sometimes emit a preamble like "Based on my exploration…" before the
 * `# Title` line. This function removes everything up to (but not including)
 * the first line that starts with `#`.
 */
export function stripPreamble(text: string): string {
  const lines = text.split('\n');
  const headingIndex = lines.findIndex(l => l.startsWith('#'));
  if (headingIndex <= 0) return text;
  return lines.slice(headingIndex).join('\n');
}
