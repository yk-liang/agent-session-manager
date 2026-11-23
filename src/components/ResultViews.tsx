import { unifiedDiff } from "../diff";
import type { ContentBlock } from "../types";
import DiffView from "./DiffView";
import MessageText from "./MessageText";
import { plainResultText, TextPreview, textView, type View } from "./ToolContent";

export function diffResult(result?: ContentBlock): View | undefined {
  return diffText(result && plainResultText(result));
}

export function diffText(text?: string): View | undefined {
  if (!text) return;
  const patch = unifiedDiff(text);
  if (patch) return { label: patch.incomplete ? "改动片段" : "改动", body: <DiffView files={patch.files} /> };
}

export function jsonResult(result?: ContentBlock): View | undefined {
  const text = result && plainResultText(result)?.trim();
  if (!text || text.length > 500_000 || !["{", "["].includes(text[0])) return;
  try {
    JSON.parse(text);
    const tokens = text.match(/"(?:\\.|[^"\\])*"|[{}\[\],:]|[^\s{}\[\],:]+/g) ?? [];
    const lines: string[] = [];
    let depth = 0, line = "";
    const newline = () => { lines.push(line); line = "  ".repeat(depth); };
    for (const [i, token] of tokens.entries()) {
      switch (token) {
        case "{": case "[":
          line += token;
          if (++depth > 32) return;
          if (tokens[i + 1] !== (token === "{" ? "}" : "]")) newline();
          break;
        case "}": case "]":
          depth--;
          if (tokens[i - 1] !== (token === "}" ? "{" : "[")) newline();
          line += token;
          break;
        case ",": line += token; newline(); break;
        case ":": line += ": "; break;
        default: line += token;
      }
    }
    const formatted = [...lines, line].join("\n");
    if (formatted !== text) return textView("JSON", formatted, null);
  } catch {
    return;
  }
}

function sources(content: unknown) {
  if (!Array.isArray(content) || !content.length) return;
  const links = content.map(entry => {
    const url = entry?.url;
    if (typeof url !== "string" || /[\s<>]/.test(url) || !/^https?:\/\//i.test(url)) return;
    const title = (typeof entry.title === "string" && entry.title || url).replace(/\s+/g, " ").replace(/[\\\[\]]/g, "\\$&");
    return `- [${title}](<${url}>)`;
  });
  if (links.every(Boolean)) return links.join("\n");
}

function markdownView(label: string, text: string): View {
  return { label, copy: text, body: <TextPreview label={label} text={text}>{text =>
    <MessageText text={text} query="" markdown fallback={<pre className="code">{text}</pre>} />
  }</TextPreview> };
}

export function documentResult(result?: ContentBlock): View | undefined {
  const text = result && plainResultText(result);
  if (text && /^(?:#{1,6} \S|\x60{3}\w+|~{3}\w+)/m.test(text)) return markdownView("阅读", text);
}

export function webResult(result?: ContentBlock, page?: "fetch" | "find"): View | undefined {
  if (!result) return;
  if (page) {
    const content = result.content;
    if (Array.isArray(content)) {
      const excerpts = content.flatMap(entry => typeof entry?.snippet === "string" && entry.snippet.trim()
        ? [[entry.title, entry.snippet, entry.url].filter(field => typeof field === "string" && field.trim()).join("\n\n")] : []);
      if (excerpts.length) return textView(page === "find" ? "页内匹配" : "网页摘要", excerpts.join("\n\n"), null);
    }
    const document = documentResult(result);
    if (document) return document;
  }
  const links = sources(result.content);
  if (links) return markdownView("来源", links);
  const text = plainResultText(result);
  if (text && /^#{1,4} \[[^\]\n]+\]\(https?:\/\//m.test(text)) return markdownView("阅读", text);
}
