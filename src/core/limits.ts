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
  scannerInputBytes: 160 * 1024 * 1024,
  delimiterRuns: 655_360,
  delimiterRunCharacters: 8,
  formulasPerAnswer: 5000,
  charactersPerFormula: 80_000,
  aggregateFormulaCharacters: 2_000_000,
  responseDocumentBytes: 40 * 1024 * 1024,
  responseDocumentLines: 400_000,
  responseDocumentBlocks: 40_960,
  renderDurationMs: 8000,
  imageWidthPx: 4096,
  imageHeightPx: 16_384,
  imagePixels: 32 * 1024 * 1024,
  rawPngBytes: 512 * 1024,
  base64PayloadBytes: 700 * 1024
});
