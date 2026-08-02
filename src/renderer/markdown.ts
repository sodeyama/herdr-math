import { createRequire } from "node:module";

import hljs from "highlight.js";
import katex from "katex";
import MarkdownIt from "markdown-it";

import { HerdrMathError } from "../core/errors.js";
import type { RendererDocumentSegment } from "./document.js";

const require = createRequire(import.meta.url);

const DENIED_COMMAND = /\\(?:href|url|includegraphics|htmlClass|htmlId|htmlStyle|htmlData)(?=[^A-Za-z]|$)/;

const PLACEHOLDER_START = "\uE000herdrMath\uE001";
const PLACEHOLDER_END = "\uE002endMath\uE003";

const markdown = createMarkdown();

function createMarkdown(): MarkdownIt {
  const instance = new MarkdownIt({
    html: false,
    linkify: false,
    typographer: false,
    breaks: false,
    highlight: (code, language) => highlightCode(code, language)
  });
  overrideImage(instance);
  overrideLink(instance);
  return instance;
}

function highlightCode(code: string, language: string): string {
  if (language !== "" && hljs.getLanguage(language)) {
    try {
      return hljs.highlight(code, { language, ignoreIllegals: true }).value;
    } catch {
      return escapeHtml(code);
    }
  }
  return escapeHtml(code);
}

function overrideImage(instance: MarkdownIt): void {
  instance.renderer.rules.image = (tokens, index) => {
    const text = tokens[index]?.content ?? "";
    return `<span class="markdown-image" aria-label="${escapeAttr(text)}">${escapeHtml(text)}</span>`;
  };
}

function overrideLink(instance: MarkdownIt): void {
  const renderer = instance.renderer.rules;
  renderer.link_open = () => '<span class="markdown-link">';
  renderer.link_close = () => "</span>";
}

export function renderMarkup(segments: readonly RendererDocumentSegment[]): string {
  const mathRenders: string[] = [];
  let source = "";

  for (const segment of segments) {
    if (segment.kind === "text") {
      source += segment.text;
      continue;
    }
    if (segment.latex.includes("\0") || DENIED_COMMAND.test(segment.latex)) {
      throw new HerdrMathError("invalid_latex");
    }
    const rendered = katex.renderToString(segment.latex, {
      displayMode: segment.display,
      throwOnError: true,
      trust: false,
      strict: (code) => (code === "unicodeTextInMathMode" ? "ignore" : "error"),
      maxSize: 50,
      maxExpand: 1000,
      macros: {},
      output: "html"
    });
    const index = mathRenders.length;
    mathRenders.push(
      segment.display ? `<span class="math-display">${rendered}</span>` : `<span class="math-inline">${rendered}</span>`
    );
    source += `${PLACEHOLDER_START}${index}${PLACEHOLDER_END}`;
  }

  let html = markdown.render(source);
  for (let index = 0; index < mathRenders.length; index += 1) {
    html = html.split(`${PLACEHOLDER_START}${index}${PLACEHOLDER_END}`).join(mathRenders[index]);
  }
  return html;
}

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

function escapeAttr(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

export const MARKDOWN_CSS_PATH = require.resolve("highlight.js/styles/github-dark.min.css");
