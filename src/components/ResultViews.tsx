import { unifiedDiff } from "../diff";
import { attachmentLabel, isAttachment } from "../sessionContent";
import type { ContentBlock } from "../types";
import DiffView from "./DiffView";
import { Attachment } from "./MessageParts";
import MessageText from "./MessageText";
import { payloadText, terminalText, TextPreview, textView, type View } from "./ToolContent";

type ResultPart = { attachment?: ContentBlock; text: string };

function resultParts(content: unknown): ResultPart[] {
  if (!Array.isArray(content)) return content == null ? [] : [{ text: payloadText(content) }];
  if (content.length && !content.some(part => part && typeof part === "object"
    && (["text", "input_text", "output_text"].includes(part.type) || isAttachment(part)))) {
    return [{ text: payloadText(content) }];
  }
  return content.map(part => part && typeof part === "object" && isAttachment(part as ContentBlock)
    ? { attachment: part as ContentBlock, text: attachmentLabel(part as ContentBlock) }
    : { text: typeof (part as ContentBlock)?.text === "string" ? (part as ContentBlock).text! : payloadText(part) });
}

/** 结果的纯文本；附件以名称代替。 */
export function resultText(result: ContentBlock) {
  return resultParts(result.content).map(part => part.text).join("\n\n");
}

export function resultHasAttachment(result?: ContentBlock) {
  return !!result && resultParts(result.content).some(part => !!part.attachment);
}

export function plainResultText(result: ContentBlock) {
  const parts = resultParts(result.content);
  if (!parts.some(part => part.attachment)) return result.output ?? parts.map(part => part.text).join("\n\n");
}

/** 结果原文；混有图片等附件时逐段展示。 */
export function resultView(result: ContentBlock, label: string, start: number | null = null): View {
  return partsView(resultParts(result.content), label, start);
}

function partsView(parts: ResultPart[], label: string, start: number | null): View {
  const text = parts.map(part => part.text).join("\n\n");
  if (!parts.some(part => part.attachment)) return textView(label, text, start);
  return { label, copy: text, body: <div className="tool-result-parts">{parts.map((part, index) => part.attachment
    ? <Attachment key={index} block={part.attachment} />
    : <TextPreview key={index} text={part.text} label={label}>{text => <pre className="code">{text}</pre>}</TextPreview>)}</div> };
}

function executionOutput(value: unknown): string | undefined {
  if (Array.isArray(value)) {
    const results = value.map(entry => executionOutput(entry));
    if (results.length && results.every(result => result !== undefined)) return results.filter(Boolean).join("\n\n");
  } else if (value && typeof value === "object") {
    const result = value as Record<string, unknown>;
    if (result.status === "fulfilled") return executionOutput(result.value);
    if (result.status === "rejected" && "reason" in result) {
      return ["Promise 被拒绝", payloadText(result.reason)].filter(Boolean).join("\n");
    }
    if (!Object.keys(result).length) return "";
    if (typeof result.output !== "string" || !["chunk_id", "exit_code", "session_id", "wall_time_seconds"].some(key => key in result)) return;
    const status = [
      typeof result.exit_code === "number" && result.exit_code !== 0 && `退出码 ${result.exit_code}`,
      typeof result.session_id === "number" && `后台会话 ${result.session_id}`,
    ].filter(Boolean).join(" · ");
    return [status, result.output.replace(/\n$/, "")].filter(Boolean).join("\n");
  }
}

function scriptOutput(text: string) {
  const decode = (text: string) => {
    const first = text.trimStart()[0];
    if (first !== "{" && first !== "[") return;
    try {
      return executionOutput(JSON.parse(text));
    } catch {
      return;
    }
  };
  const entire = decode(text);
  if (entire !== undefined) return entire;
  const readable = text.split("\n").map(line => decode(line) ?? line).join("\n");
  if (readable !== text) return readable;
}

export function scriptView(result?: ContentBlock, label = "结果", terminal = false): View | undefined {
  const parts = resultParts(result?.content);
  let changed = false;
  const decoded = parts.flatMap((part, index) => {
    if (part.attachment) return [part];
    const stripped = index === 0
      ? part.text.replace(/^Script completed(?:\r?\nWall time:? [^\r\n]+)?(?:\r?\nOutput:[ \t]*(?:\r?\n|$)|\s*$)/, "")
      : part.text;
    const parsed = scriptOutput(stripped) ?? stripped;
    const output = terminal ? terminalText(parsed) : parsed;
    changed ||= output !== part.text;
    return output ? [{ text: output }] : [];
  });
  if (changed) return partsView(decoded, label, null);
}

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
  if (page && Array.isArray(result.content)) {
    const excerpts = result.content.flatMap(entry => typeof entry?.snippet === "string" && entry.snippet.trim()
      ? [[entry.title, entry.snippet, entry.url].filter(field => typeof field === "string" && field.trim()).join("\n\n")] : []);
    if (excerpts.length) return textView(page === "find" ? "页内匹配" : "网页摘要", excerpts.join("\n\n"), null);
  }
  const document = page && documentResult(result);
  if (document) return document;
  const links = sources(result.content);
  if (links) return markdownView("来源", links);
  return page ? undefined : documentResult(result);
}
