import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { attachmentLabel, isAttachment } from "../sessionContent";
import type { ContentBlock } from "../types";
import { Attachment } from "./MessageParts";

export type View = { label: string; body: ReactNode; copy?: string; meta?: string };

export function payloadText(value: unknown) {
  return typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? "";
}

/** shell 语法着色：每段的程序名、参数、字符串和操作符。 */
export function ShellText({ command }: { command: string }) {
  const tokens = command.match(/'(?:[^']*)'|"(?:\\.|[^"\\])*"|&&|\|\||[|;<>]|\s+|[^\s|;&<>'"]+|[\s\S]/g) ?? [];
  let program = true;
  return <>{tokens.map((token, i) => {
    const operator = /^(?:&&|\|\||[|;<>])$/.test(token);
    const kind = operator ? "operator" : /^['"]/.test(token) ? "string" : /^-{1,2}\w/.test(token) ? "flag"
      : program && /^[^\s=]+$/.test(token) ? "program" : undefined;
    if (operator || token.includes("\n")) program = !/^[<>]$/.test(token);
    else if (kind === "program") program = false;
    return kind ? <span className={`tool-token-${kind}`} key={i}>{token}</span> : token;
  })}</>;
}

export function commandView(command?: string): View | undefined {
  if (!command) return;
  return { label: "命令", copy: command, body: <TextPreview label="命令" text={command}>{text =>
    <div className="tool-command"><span aria-hidden="true">$</span><pre className="code"><ShellText command={text} /></pre></div>
  }</TextPreview> };
}

const OUTPUT_PREVIEW_LIMIT = 24_000;

/** 末尾换行不算作新的一行。 */
function countLines(text: string) {
  let count = text.endsWith("\n") ? 0 : 1;
  for (let offset = text.indexOf("\n"); offset !== -1; offset = text.indexOf("\n", offset + 1)) count++;
  return count;
}

export function ExpandableFrame({ lines, truncated = false, label, children }: {
  lines?: number;
  truncated?: boolean;
  label: string;
  children: (expanded: boolean) => ReactNode;
}) {
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);
  const viewport = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const frame = viewport.current;
    const body = content.current;
    if (expanded || !frame || !body) return;
    const measure = () => setOverflows(frame.scrollHeight > frame.clientHeight);
    const observer = new ResizeObserver(measure);
    measure();
    observer.observe(frame);
    observer.observe(body);
    return () => observer.disconnect();
  }, [expanded]);
  const capped = truncated || overflows;
  return <>
    <div className="tool-output-frame" data-capped={capped && !expanded || undefined}>
      <div ref={viewport} className="tool-output" data-expanded={expanded} tabIndex={0} role="region" aria-label={label}>
        <div ref={content} className="tool-output-content">{children(expanded)}</div>
      </div>
    </div>
    {(capped || expanded) && <button type="button" className="content-expand btn-sm" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
      {expanded ? "收起" : lines && lines > 1 ? `展开全部 ${lines} 行` : "展开全部"}
    </button>}
  </>;
}

export function TextPreview({ text, label, children }: {
  text: string;
  label: string;
  children: (text: string) => ReactNode;
}) {
  const truncated = text.length > OUTPUT_PREVIEW_LIMIT;
  return <ExpandableFrame lines={text ? countLines(text) : 0} truncated={truncated} label={label}>
    {expanded => children(truncated && !expanded ? text.slice(0, OUTPUT_PREVIEW_LIMIT) : text)}
  </ExpandableFrame>;
}

export function plainView(label: string, text?: string, code = false): View | undefined {
  if (text) return { label, copy: text, body: <TextPreview text={text} label={label}>{text =>
    code ? <pre className="code">{text}</pre> : <p className="tool-payload-message tool-plain">{text}</p>
  }</TextPreview> };
}

/** 输出原文；`start` 为首行行号，为空时不显示行号（命令输出、搜索结果）。 */
function OutputText({ text, start, label }: { text: string; start: number | null; label: string }) {
  if (!text) return <p className="tool-loading tool-payload-message">无{label}</p>;
  return <TextPreview text={text} label={label}>
    {displayed => {
      const lines = start === null ? undefined : Array.from({ length: countLines(displayed) }, (_, i) => i + start).join("\n");
      return <div className="tool-output-lines" data-numbered={!!lines || undefined}>
        {lines && <pre className="tool-line-numbers" aria-hidden="true">{lines}</pre>}
        <pre className="code">{displayed}</pre>
      </div>;
    }}
  </TextPreview>;
}

/** 文本形式的展示方式，附带行数。 */
export function textView(label: string, text: string, start: number | null = 1): View {
  const lineCount = text ? countLines(text) : 0;
  return { label, body: <OutputText text={text} start={start} label={label} />, copy: text || undefined, meta: lineCount ? `${lineCount} 行` : undefined };
}

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

export function plainResultText(result: ContentBlock) {
  const parts = resultParts(result.content);
  if (!parts.some(part => part.attachment)) return result.command_output ?? parts.map(part => part.text).join("\n\n");
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

export function scriptView(result?: ContentBlock, label = "结果"): View | undefined {
  const parts = resultParts(result?.content);
  let changed = false;
  const decoded = parts.flatMap((part, index) => {
    if (part.attachment) return [part];
    const text = index === 0 ? part.text.replace(/^Script completed\r?\n(?:Wall time [^\r\n]+\r?\n)?Output:[ \t]*(?:\r?\n|$)/, "") : part.text;
    const output = scriptOutput(text) ?? text;
    changed ||= output !== part.text;
    return output ? [{ text: output }] : [];
  });
  if (changed) return partsView(decoded, label, null);
}
