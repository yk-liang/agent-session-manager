import { useState, type ReactNode } from "react";
import { attachmentLabel, isAttachment } from "../sessionContent";
import type { ContentBlock } from "../types";
import { Attachment, LongText } from "./MessageParts";

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

function CommandText({ command }: { command: string }) {
  return <div className="tool-command"><span aria-hidden="true">$</span><pre className="code"><ShellText command={command} /></pre></div>;
}

export function commandView(command?: string): View | undefined {
  if (command) return { label: "命令", body: <CommandText command={command} />, copy: command };
}

const OUTPUT_PREVIEW_LIMIT = 24_000;

/** 末尾换行不算作新的一行。 */
function countLines(text: string) {
  let count = text.endsWith("\n") ? 0 : 1;
  for (let offset = text.indexOf("\n"); offset !== -1; offset = text.indexOf("\n", offset + 1)) count++;
  return count;
}

/** 超过 12 行时限制高度，展开后显示全部；`overflow` 表示内容还被截断成了预览。 */
export function ExpandableFrame({ lines, overflow = false, label, children }: {
  lines: number;
  overflow?: boolean;
  label: string;
  children: (expanded: boolean) => ReactNode;
}) {
  const [expanded, setExpanded] = useState(false);
  const capped = lines > 12 || overflow;
  return <>
    <div className="tool-output-frame" data-capped={capped && !expanded || undefined}>
      <div className="tool-output" data-expanded={expanded} tabIndex={0} role="region" aria-label={label}>
        {children(expanded)}
      </div>
    </div>
    {capped && <button type="button" className="content-expand btn-sm" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
      {expanded ? "收起" : `展开全部 ${lines} 行`}
    </button>}
  </>;
}

/** 输出原文；`start` 为首行行号，为空时不显示行号（命令输出、搜索结果）。 */
function OutputText({ text, start, label, lineCount }: { text: string; start: number | null; label: string; lineCount: number }) {
  if (!text) return <p className="tool-loading tool-payload-message">无{label}</p>;
  return <ExpandableFrame lines={lineCount} overflow={text.length > OUTPUT_PREVIEW_LIMIT} label={label}>
    {expanded => {
      const displayed = expanded || text.length <= OUTPUT_PREVIEW_LIMIT ? text : text.slice(0, OUTPUT_PREVIEW_LIMIT);
      const lines = start === null ? undefined : Array.from({ length: countLines(displayed) }, (_, i) => i + start).join("\n");
      return <div className="tool-output-lines" data-numbered={!!lines || undefined}>
        {lines && <pre className="tool-line-numbers" aria-hidden="true">{lines}</pre>}
        <pre className="code">{displayed}</pre>
      </div>;
    }}
  </ExpandableFrame>;
}

/** 文本形式的展示方式，附带行数。 */
export function textView(label: string, text: string, start: number | null = 1): View {
  const lineCount = text ? countLines(text) : 0;
  return { label, body: <OutputText text={text} start={start} label={label} lineCount={lineCount} />, copy: text || undefined, meta: lineCount ? `${lineCount} 行` : undefined };
}

type ResultPart = { attachment?: ContentBlock; text: string };

function resultParts(content: unknown): ResultPart[] {
  if (!Array.isArray(content)) return content == null ? [] : [{ text: payloadText(content) }];
  return content.map(part => part && typeof part === "object" && isAttachment(part as ContentBlock)
    ? { attachment: part as ContentBlock, text: attachmentLabel(part as ContentBlock) }
    : { text: typeof (part as ContentBlock)?.text === "string" ? (part as ContentBlock).text! : payloadText(part) });
}

/** 结果的纯文本；附件以名称代替。 */
export function resultText(result: ContentBlock) {
  return resultParts(result.content).map(part => part.text).join("\n\n");
}

/** 结果原文；混有图片等附件时逐段展示。 */
export function resultView(result: ContentBlock, label: string, start: number | null = null): View {
  const parts = resultParts(result.content);
  const text = parts.map(part => part.text).join("\n\n");
  if (!parts.some(part => part.attachment)) return textView(label, text, start);
  return { label, copy: text, body: <div className="tool-result-parts">{parts.map((part, index) => part.attachment
    ? <Attachment key={index} block={part.attachment} />
    : <LongText key={index} text={part.text} code />)}</div> };
}

/** Codex 脚本用 `text(…)` 打印各次调用的返回值（`Promise.allSettled` 的结果或工具返回的对象），取出其中的输出文本。 */
function scriptOutput(text: string) {
  let unwrapped = false;
  const lines = text.split("\n").map(line => {
    if (!line.startsWith("{")) return line;
    try {
      const value = JSON.parse(line);
      const result = value?.status === "fulfilled" ? value.value : value;
      if (typeof result?.output !== "string" || !["chunk_id", "exit_code", "session_id", "wall_time_seconds"].some(key => key in result)) return line;
      unwrapped = true;
      return result.output.replace(/\n$/, "");
    } catch {
      return line;
    }
  });
  return unwrapped ? lines.join("\n") : undefined;
}

export function scriptView(result?: ContentBlock, label = "结果"): View | undefined {
  const parts = resultParts(result?.content);
  if (parts.some(part => part.attachment)) return;
  const output = scriptOutput(parts.map(part => part.text).join("\n\n"));
  if (output !== undefined) return textView(label, output, null);
}
