import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import { performance } from "node:perf_hooks";
import { pathToFileURL } from "node:url";

const corpusPath = process.argv[2];
if (!corpusPath) throw new Error("Usage: node run.mjs <formula-corpus.json>");
const started = performance.now();
const require = createRequire(import.meta.url);
const katex = require("katex");
const { chromium } = require("playwright");
const sharp = require("sharp");
const corpus = JSON.parse(readFileSync(corpusPath, "utf8"));
const outputDirectory = process.env.OUTPUT_DIR;
if (outputDirectory) mkdirSync(outputDirectory, { recursive: true });
const cssPath = require.resolve("katex/dist/katex.min.css");
const css = readFileSync(cssPath, "utf8");
const baseUrl = pathToFileURL(`${dirname(cssPath)}/`).href;
let remoteRequests = 0;
let peakRss = process.memoryUsage().rss;
const sampleMemory = () => {
  peakRss = Math.max(peakRss, process.memoryUsage().rss);
};

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 4096, height: 4096 } });
await context.route(/^https?:\/\//, async (route) => {
  remoteRequests += 1;
  await route.abort("blockedbyclient");
});
const page = await context.newPage();

function markup(formulas) {
  return formulas
    .map(({ latex, display }) =>
      katex.renderToString(latex, {
        displayMode: display,
        throwOnError: true,
        trust: false,
        strict: "ignore",
        maxSize: 50,
        maxExpand: 1000,
        output: "html"
      })
    )
    .join("");
}

async function render(formulas, id) {
  const html = markup(formulas);
  await page.setContent(
    `<!doctype html><html><head><base href="${baseUrl}"><style>${css}\nhtml,body{margin:0;background:#fff}#render{display:flex;flex-direction:column;gap:12px;width:max-content;max-width:4000px;padding:20px;color:#111;font-size:24px}.katex-display{margin:0}</style></head><body><div id="render">${html}</div></body></html>`
  );
  await page.evaluate(() => document.fonts.ready);
  const screenshot = await page.locator("#render").screenshot({ type: "png" });
  const result = await sharp(screenshot).png({ compressionLevel: 9 }).toBuffer({ resolveWithObject: true });
  if (outputDirectory && id) writeFileSync(`${outputDirectory}/${id}.png`, result.data);
  sampleMemory();
  return { bytes: result.data.byteLength, width: result.info.width, height: result.info.height };
}

const cases = [];
let coldEndToEndMs = 0;
for (const testCase of corpus.validCases) {
  const caseStarted = performance.now();
  const image = await render(testCase.formulas, testCase.id);
  cases.push({ id: testCase.id, durationMs: performance.now() - caseStarted, ...image });
  if (cases.length === 1) coldEndToEndMs = performance.now() - started;
}
const invalidAccepted = [];
for (const testCase of corpus.invalidCases) {
  try {
    markup([testCase.formula]);
    invalidAccepted.push(testCase.id);
  } catch {}
}
for (const testCase of corpus.securityCases) await render([testCase.formula]);
const holdMs = Number(process.env.HOLD_MS ?? 0);
if (Number.isFinite(holdMs) && holdMs > 0) await new Promise((resolve) => setTimeout(resolve, holdMs));

await page.close();
await context.close();
await browser.close();
sampleMemory();
const warm = cases
  .slice(1)
  .map(({ durationMs }) => durationMs)
  .sort((left, right) => left - right);
const pngBytes = cases.map(({ bytes }) => bytes);
process.stdout.write(
  `${JSON.stringify(
    {
      candidate: "katex-playwright-sharp",
      versions: {
        katex: require("katex/package.json").version,
        playwright: require("playwright/package.json").version,
        sharp: sharp.versions.sharp
      },
      startupAndCorpusMs: performance.now() - started,
      coldEndToEndMs,
      coldRenderMs: cases[0]?.durationMs ?? 0,
      warmMedianMs: warm[Math.floor(warm.length / 2)] ?? 0,
      peakNodeRssBytes: peakRss,
      pngBytesTotal: pngBytes.reduce((sum, value) => sum + value, 0),
      pngBytesMedian: [...pngBytes].sort((left, right) => left - right)[Math.floor(pngBytes.length / 2)] ?? 0,
      validPassed: cases.length,
      invalidRejected: corpus.invalidCases.length - invalidAccepted.length,
      invalidAccepted,
      remoteRequests,
      cases,
      cleanup: { browserClosed: true, pageClosed: true, contextClosed: true }
    },
    null,
    2
  )}\n`
);
