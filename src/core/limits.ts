export interface PolicyLimits {
  eventJsonBytes: number;
  paneReadLines: number;
  paneReadBytes: number;
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
  anchorOccurrences: number;
  boundaryCandidates: number;
  stateFileBytes: number;
  socketResponseBytes: number;
  staleLockAgeMs: number;
  fingerprintExpiryMs: number;
}

export const POLICY_LIMITS: Readonly<PolicyLimits> = Object.freeze({
  eventJsonBytes: 64 * 1024,
  paneReadLines: 1000,
  paneReadBytes: 1024 * 1024,
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
  base64PayloadBytes: 700 * 1024,
  anchorOccurrences: 256,
  boundaryCandidates: 2048,
  stateFileBytes: 64 * 1024,
  socketResponseBytes: 2 * 1024 * 1024,
  staleLockAgeMs: 120_000,
  fingerprintExpiryMs: 24 * 60 * 60 * 1000
});
