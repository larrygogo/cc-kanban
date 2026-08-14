import { describe, expect, it } from "vitest";
import { escapeHtml, highlightCode, highlightLines, languageForPath } from "./highlight";

describe("languageForPath", () => {
  it("maps common extensions to registered languages", () => {
    expect(languageForPath("src/App.tsx")).toBe("typescript");
    expect(languageForPath("a/b/index.mjs")).toBe("javascript");
    expect(languageForPath("main.rs")).toBe("rust");
    expect(languageForPath("scripts/deploy.SH")).toBe("bash");
    expect(languageForPath("config.yaml")).toBe("yaml");
    expect(languageForPath("page.html")).toBe("xml");
    expect(languageForPath("README.md")).toBe("markdown");
    expect(languageForPath("Cargo.toml")).toBe("ini");
    expect(languageForPath("Program.cs")).toBe("csharp");
    expect(languageForPath("fix.patch")).toBe("diff");
  });

  it("handles Dockerfile by filename", () => {
    expect(languageForPath("Dockerfile")).toBe("dockerfile");
    expect(languageForPath("build/Dockerfile.dev")).toBe("dockerfile");
  });

  it("returns null for unknown, missing, or dotfile-only extensions", () => {
    expect(languageForPath("notes.xyz")).toBeNull();
    expect(languageForPath("Makefile")).toBeNull();
    expect(languageForPath(".gitignore")).toBeNull();
    expect(languageForPath("")).toBeNull();
  });
});

describe("highlightCode", () => {
  it("wraps tokens in hljs spans for known languages", () => {
    const html = highlightCode("const x = 1;", "javascript");
    expect(html).toContain("hljs-");
    expect(html).toContain("const");
  });

  it("falls back to escaped plain text for null or unknown languages", () => {
    expect(highlightCode("a < b && \"c\"", null)).toBe("a &lt; b &amp;&amp; &quot;c&quot;");
    expect(highlightCode("<b>x</b>", "no-such-lang")).toBe("&lt;b&gt;x&lt;/b&gt;");
  });
});

describe("highlightLines", () => {
  it("keeps output line count equal to input", () => {
    const html = highlightLines(["let a = 1", "let b = 2"], "rust");
    expect(html).toHaveLength(2);
    expect(html[0]).toContain("hljs-");
  });

  it("carries multi-line token state across lines (block comment)", () => {
    const html = highlightLines(["/* first", "second */", "let x = 1;"], "javascript");
    expect(html).toHaveLength(3);
    // 逐行独立高亮时第二行会丢注释态——整篇高亮后必须仍是注释色。
    expect(html[1]).toContain("hljs-comment");
    expect(html[2]).toContain("hljs-keyword");
  });

  it("balances span tags within every line of a multi-line string", () => {
    const html = highlightLines(["const s = `first", "second`;"], "javascript");
    expect(html).toHaveLength(2);
    for (const line of html) {
      const open = (line.match(/<span/g) ?? []).length;
      const close = (line.match(/<\/span>/g) ?? []).length;
      expect(open).toBe(close);
    }
    expect(html[1]).toContain("hljs-string");
  });

  it("preserves empty lines", () => {
    const html = highlightLines(["let a = 1;", "", "let b = 2;"], "javascript");
    expect(html).toHaveLength(3);
    expect(html[1]).toBe("");
  });

  it("escapes plain text lines when lang is null", () => {
    expect(highlightLines(["<div>"], null)).toEqual(["&lt;div&gt;"]);
  });
});

describe("escapeHtml", () => {
  it("escapes &, <, > and quotes", () => {
    expect(escapeHtml(`<a href="x">&`)).toBe("&lt;a href=&quot;x&quot;&gt;&amp;");
  });
});
