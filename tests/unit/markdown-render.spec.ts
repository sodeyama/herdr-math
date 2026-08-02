import { describe, expect, it } from "vitest";

import { renderMarkup } from "../../src/renderer/markdown.js";
import type { RendererDocumentSegment } from "../../src/renderer/document.js";

function text(value: string): RendererDocumentSegment {
  return Object.freeze({ kind: "text", text: value });
}

function math(latex: string, display = false): RendererDocumentSegment {
  return Object.freeze({ kind: "math", latex, display });
}

describe("markdown response rendering", () => {
  it("renders a GitHub-style table with aligned cells", () => {
    const html = renderMarkup([text("Result:\n\n| Name | Value |\n|---|---|\n| alpha | 1 |\n| beta | 2 |")]);
    expect(html).toContain("<table>");
    expect(html).toContain("<th>Name</th>");
    expect(html).toContain("<th>Value</th>");
    expect(html).toContain("<td>alpha</td>");
    expect(html).toContain("<td>1</td>");
  });

  it("renders headings, emphasis, lists, and quotes", () => {
    const html = renderMarkup([
      text("# Title\n\nSome **bold** and *italic*.\n\n- item one\n- item two\n\n> a quote\n\n---")
    ]);
    expect(html).toContain("<h1>Title</h1>");
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<em>italic</em>");
    expect(html).toContain("<ul>");
    expect(html).toContain("<blockquote>");
    expect(html).toContain("<hr>");
  });

  it("highlights fenced code blocks with a language", () => {
    const html = renderMarkup([text("```js\nconst x = 1;\n```")]);
    expect(html).toContain('<code class="language-js">');
    expect(html).toContain('class="hljs');
  });

  it("escapes raw HTML instead of executing it", () => {
    const html = renderMarkup([text("Before <script>alert(1)</script> and <img src=x onerror=y>")]);
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;img");
    expect(html).not.toMatch(/<(script|img)[\s>]/);
  });

  it("renders links as inert text without an href or active anchor", () => {
    const output = renderMarkup([text("See [docs](https://example.com/page)")]);
    expect(output).not.toContain("href=");
    expect(output).toContain("docs");
  });

  it("renders images as inert alt text without a src", () => {
    const output = renderMarkup([text("![alt](https://example.com/x.png)")]);
    expect(output).not.toContain("src=");
    expect(output).toContain("alt");
  });

  it("interleaves math placeholders with markdown source order", () => {
    const html = renderMarkup([
      text("Sum "),
      math("x^2", false),
      text(" is here.\n\n| A |\n|---|\n| "),
      math("y_1", false),
      text(" |")
    ]);
    expect(html.indexOf("Sum")).toBeLessThan(html.indexOf('class="math-inline"'));
    expect(html).toContain('<td><span class="math-inline">');
    expect(html).toContain(">y</span>");
    expect(html).toContain(">x</span>");
  });

  it("keeps display math as a block span", () => {
    const html = renderMarkup([text("\n\n"), math("a^2+b^2=c^2", true), text("\n\nAfter.")]);
    expect(html).toContain('<span class="math-display">');
  });

  it("rejects denied latex commands before rendering", () => {
    expect(() => renderMarkup([text("before "), math("\\href{https://x}{y}")])).toThrow();
  });
});
