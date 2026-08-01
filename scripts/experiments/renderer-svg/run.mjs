import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { performance } from "node:perf_hooks";

const corpusPath = process.argv[2];
if (!corpusPath) throw new Error("Usage: node run.mjs <formula-corpus.json>");
const started = performance.now();
const require = createRequire(import.meta.url);
let networkAttempts = 0;
const denyNetwork = () => {
  networkAttempts += 1;
  throw new Error("Network access denied by renderer experiment.");
};
for (const moduleName of ["node:http", "node:https"]) {
  const module = require(moduleName);
  module.request = denyNetwork;
  module.get = denyNetwork;
}
const net = require("node:net");
const tls = require("node:tls");
const dns = require("node:dns");
net.connect = denyNetwork;
net.createConnection = denyNetwork;
tls.connect = denyNetwork;
dns.lookup = denyNetwork;

globalThis.MathJax = {
  loader: {
    paths: { mathjax: "@mathjax/src/bundle" },
    load: ["adaptors/liteDOM"],
    require: (file) => import(file)
  },
  output: { font: "mathjax-newcm" },
  tex: {
    formatError(_jax, error) {
      throw error;
    }
  }
};
await import("@mathjax/src/bundle/tex-svg.js");
await globalThis.MathJax.startup.promise;
const { Resvg } = require("@resvg/resvg-js");
const corpus = JSON.parse(readFileSync(corpusPath, "utf8"));
const outputDirectory = process.env.OUTPUT_DIR;
if (outputDirectory) mkdirSync(outputDirectory, { recursive: true });
let peakRss = process.memoryUsage().rss;
const sampleMemory = () => {
  peakRss = Math.max(peakRss, process.memoryUsage().rss);
};

async function svg(latex, display) {
  const node = await globalThis.MathJax.tex2svgPromise(latex, {
    display,
    em: 16,
    ex: 8,
    containerWidth: 1280
  });
  const adaptor = globalThis.MathJax.startup.adaptor;
  const element = adaptor.tags(node, "svg")[0];
  if (element === undefined) throw new Error("MathJax returned no SVG element.");
  return adaptor.serializeXML(element);
}

async function render(formula, id) {
  const source = await svg(formula.latex, formula.display);
  if (/(?:href|src)\s*=\s*["'](?:https?:|file:)|<image\b|<foreignObject\b/i.test(source)) {
    throw new Error("Unsafe external SVG reference.");
  }
  const renderer = new Resvg(source, { background: "white", fitTo: { mode: "zoom", value: 2 } });
  const image = renderer.render();
  const png = image.asPng();
  if (outputDirectory && id) writeFileSync(`${outputDirectory}/${id}.png`, png);
  sampleMemory();
  return { bytes: png.byteLength, width: image.width, height: image.height };
}

const cases = [];
let coldEndToEndMs = 0;
for (const testCase of corpus.validCases) {
  const caseStarted = performance.now();
  const images = [];
  for (const [index, formula] of testCase.formulas.entries()) {
    images.push(await render(formula, `${testCase.id}-${index}`));
  }
  cases.push({
    id: testCase.id,
    durationMs: performance.now() - caseStarted,
    bytes: images.reduce((sum, image) => sum + image.bytes, 0),
    width: Math.max(...images.map((image) => image.width)),
    height: images.reduce((sum, image) => sum + image.height, 0)
  });
  if (cases.length === 1) coldEndToEndMs = performance.now() - started;
}
const invalidAccepted = [];
for (const testCase of corpus.invalidCases) {
  try {
    await svg(testCase.formula.latex, testCase.formula.display);
    invalidAccepted.push(testCase.id);
  } catch {}
}
const securityAccepted = [];
for (const testCase of corpus.securityCases) {
  try {
    await render(testCase.formula);
    securityAccepted.push(testCase.id);
  } catch {}
}

globalThis.MathJax.done();
sampleMemory();
const warm = cases
  .slice(1)
  .map(({ durationMs }) => durationMs)
  .sort((left, right) => left - right);
const pngBytes = cases.map(({ bytes }) => bytes);
process.stdout.write(
  `${JSON.stringify(
    {
      candidate: "mathjax-svg-resvg",
      versions: {
        mathjax: require("@mathjax/src/package.json").version,
        mathjaxFont: require("@mathjax/mathjax-newcm-font/package.json").version,
        resvg: require("@resvg/resvg-js/package.json").version
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
      securityRejected: corpus.securityCases.length - securityAccepted.length,
      securityAccepted,
      networkAttempts,
      cases,
      cleanup: { mathJaxDoneCalled: true }
    },
    null,
    2
  )}\n`
);
