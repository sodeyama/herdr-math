export interface PolicyLimits {
  scannerInputBytes: number;
  delimiterRuns: number;
  delimiterRunCharacters: number;
  formulasPerAnswer: number;
  charactersPerFormula: number;
  aggregateFormulaCharacters: number;
  responseDocumentBytes: number;
  responseDocumentLines: number;
  responseDocumentBlocks: number;
  renderDurationMs: number;
  imageWidthPx: number;
  imageHeightPx: number;
  imagePixels: number;
  rawPngBytes: number;
  base64PayloadBytes: number;
}

export const POLICY_LIMITS: Readonly<PolicyLimits> = Object.freeze({
  scannerInputBytes: 1024 * 1024,
  delimiterRuns: 4096,
  delimiterRunCharacters: 8,
  formulasPerAnswer: 20,
  charactersPerFormula: 2000,
  aggregateFormulaCharacters: 10_000,
  responseDocumentBytes: 256 * 1024,
  responseDocumentLines: 4000,
  responseDocumentBlocks: 512,
  renderDurationMs: 8000,
  imageWidthPx: 4096,
  imageHeightPx: 16_384,
  imagePixels: 32 * 1024 * 1024,
  rawPngBytes: 512 * 1024,
  base64PayloadBytes: 700 * 1024
});
