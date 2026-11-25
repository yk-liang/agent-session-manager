import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { displayPath } from "../utils";

export type View = { label: string; body: ReactNode; copy?: string; meta?: string; plain?: boolean };

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

type Command = { command: string; cwd?: string };

export function commandView(command?: string): View | undefined {
  return command ? commandsView([{ command }]) : undefined;
}

function shellQuote(value: string) {
  return /^[A-Za-z0-9_./:@%+=,-]+$/.test(value) ? value : `'${value.replace(/'/g, "'\\''")}'`;
}

function scopedCommand({ command, cwd }: Command, isolate: boolean) {
  if (!cwd && !isolate) return command;
  return cwd ? `(\ncd ${shellQuote(cwd)} || exit\n${command}\n)` : `(\n${command}\n)`;
}

/** 一条或几条命令，各自带 `$`；指定了工作目录的在命令上方注明，复制时用子 shell 隔离每段目录与退出状态。 */
export function commandsView(commands: Command[], cwd?: string | null, copyable = true): View | undefined {
  if (!commands.length) return;
  const isolate = commands.length > 1;
  const copy = commands.map(entry => scopedCommand(entry, isolate)).join("\n\n");
  const truncated = copy.length > OUTPUT_PREVIEW_LIMIT;
  return { label: "命令", copy: copyable ? copy : undefined, body: <ExpandableFrame lines={countLines(copy)} truncated={truncated} label="命令">{expanded => {
    let budget = expanded ? Infinity : OUTPUT_PREVIEW_LIMIT;
    return <div className="tool-commands">{commands.map(({ command, cwd: dir }, i) => {
      const shown = command.slice(0, Math.max(budget, 0));
      budget -= command.length;
      return shown && <div key={i} className="tool-command">
        {dir && displayPath(dir, cwd) !== "." && <span className="tool-command-cwd" title={dir}># {displayPath(dir, cwd)}</span>}
        <span aria-hidden="true">$</span><pre className="code"><ShellText command={shown} /></pre>
      </div>;
    })}</div>;
  }}</ExpandableFrame> };
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
  const numbered = start !== null && !selfNumbered(text);
  return <TextPreview text={text} label={label}>
    {displayed => {
      const lines = numbered ? Array.from({ length: countLines(displayed) }, (_, i) => i + start).join("\n") : undefined;
      return <div className="tool-output-lines" data-numbered={!!lines || undefined}>
        {lines && <pre className="tool-line-numbers" aria-hidden="true">{lines}</pre>}
        <pre className="code">{displayed}</pre>
      </div>;
    }}
  </TextPreview>;
}

/** 正文已自带行号（`cat -n`、`nl`、`grep -n`）时不再另加一列。 */
function selfNumbered(text: string) {
  const lines = text.split("\n", 20).filter(line => line.trim());
  return lines.length > 0 && lines.every(line => /^\s*\d+[\t:|]/.test(line));
}

/** 终端输出里的颜色控制序列去掉，`\r` 覆盖的进度帧只留最后显示的一帧。 */
export function terminalText(text: string) {
  if (!/[\x1b\r]/.test(text)) return text;
  return text.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "").split("\n").map(line => {
    const frame = line.endsWith("\r") ? line.slice(0, -1) : line;
    return frame.slice(frame.lastIndexOf("\r") + 1);
  }).join("\n");
}

/** 文本形式的展示方式，附带行数；保持原文，不做终端控制序列清洗。 */
export function textView(label: string, text: string, start: number | null = 1): View {
  const lineCount = text ? countLines(text) : 0;
  return { label, body: <OutputText text={text} start={start} label={label} />, copy: text || undefined, meta: lineCount ? `${lineCount} 行` : undefined, plain: true };
}
