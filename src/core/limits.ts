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
  renderDurationMs: number;
  imageWidthPx: number;
  imageHeightPx: number;
  imagePixels: number;
  rawPngBytes: number;
  base64PayloadBytes: number;
  anchorOccurrences: number;
  stateFileBytes: number;
  socketResponseBytes: number;
  staleLockAgeMs: number;
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
  renderDurationMs: 8000,
  imageWidthPx: 4096,
  imageHeightPx: 16_384,
  imagePixels: 32 * 1024 * 1024,
  rawPngBytes: 512 * 1024,
  base64PayloadBytes: 700 * 1024,
  anchorOccurrences: 256,
  stateFileBytes: 64 * 1024,
  socketResponseBytes: 2 * 1024 * 1024,
  staleLockAgeMs: 120_000
});
