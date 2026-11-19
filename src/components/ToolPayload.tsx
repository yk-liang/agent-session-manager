import { useMemo, useState, type ReactNode } from "react";
import CopyButton from "./CopyButton";
import MessageText from "./MessageText";
import "../toolPayload.css";

type SectionKind = "input" | "output" | "error";
type CopyError = (error: unknown) => void;

export function payloadText(value: unknown) {
  return typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? "";
}

export function ToolSection({ kind, title, actions, children }: {
  kind: SectionKind;
  title?: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return <section className="tool-payload" data-kind={kind}>
    <div className="tool-payload-header">
      <span className="tool-payload-title">{title ?? { input: "输入", output: "输出", error: "错误输出" }[kind]}</span>
      {actions && <div className="tool-payload-actions">{actions}</div>}
    </div>
    {children}
  </section>;
}

function commandText(input: unknown): string | undefined {
  if (!input || typeof input !== "object") return;
  const fields = input as Record<string, unknown>;
  for (const command of [fields.command, fields.cmd, fields.commandLine]) {
    if (typeof command === "string" && command.trim()) return command;
    if (Array.isArray(command) && command.length && command.every(arg => typeof arg === "string")) {
      return command.map(arg => /^[\w./:=@%+,\-]+$/.test(arg) ? arg : "'" + arg.replace(/'/g, "'\\''") + "'").join(" ");
    }
  }
}

function CommandPreview({ command }: { command: string }) {
  const tokens = command.match(/'(?:[^']*)'|"(?:\\.|[^"\\])*"|&&|\|\||[|;<>]|\s+|[^\s|;&<>'"]+|[\s\S]/g) ?? [];
  return <div className="tool-command"><span aria-hidden="true">$</span><pre className="code">{tokens.map((token, i) => {
    const kind = /^(?:&&|\|\||[|;<>])$/.test(token) ? "operator"
      : /^['"]/.test(token) ? "string" : /^-{1,2}\w/.test(token) ? "flag" : undefined;
    return kind ? <span className={`tool-token-${kind}`} key={i}>{token}</span> : token;
  })}</pre></div>;
}

export function ToolInput({ value, onCopyError }: { value: unknown; onCopyError: CopyError }) {
  const [showRaw, setShowRaw] = useState(false);
  const raw = payloadText(value);
  const command = commandText(value);
  const showingCommand = !!command && !showRaw;
  return <ToolSection kind="input" title={showingCommand ? "命令" : undefined} actions={<>
    {command && <button type="button" className="content-action" onClick={() => setShowRaw(!showRaw)}>
      {showRaw ? "查看命令" : "原始 JSON"}
    </button>}
    {raw && <CopyButton className="content-action" label={showingCommand ? "复制命令" : "复制输入"}
      value={showingCommand ? command : raw} onError={onCopyError} />}
  </>}>
    {showingCommand ? <CommandPreview command={command} /> : raw
      ? <pre className="code tool-input-code">{raw}</pre>
      : <p className="tool-loading tool-payload-message">无输入</p>}
  </ToolSection>;
}

const OUTPUT_PREVIEW_LIMIT = 24_000;

function countLines(text: string) {
  let count = 1;
  for (let offset = text.indexOf("\n"); offset !== -1; offset = text.indexOf("\n", offset + 1)) count++;
  return count;
}

export function ToolOutput({ text, markdown, error, onCopyError, children }: {
  text: string;
  markdown: boolean;
  error: boolean;
  onCopyError: CopyError;
  children?: ReactNode;
}) {
  const [expanded, setExpanded] = useState(false);
  const lineCount = useMemo(() => countLines(text), [text]);
  const truncated = !children && !expanded && text.length > OUTPUT_PREVIEW_LIMIT;
  const displayed = truncated ? text.slice(0, OUTPUT_PREVIEW_LIMIT) : text;
  const lineNumbers = useMemo(() => {
    if (children) return "";
    return Array.from({ length: countLines(displayed) }, (_, i) => i + 1).join("\n");
  }, [children, displayed]);
  const showMore = !children && !!text && (lineCount > 12 || text.length > OUTPUT_PREVIEW_LIMIT);
  return <ToolSection kind={error ? "error" : "output"} actions={<>
    {!children && text && <span>{lineCount} 行</span>}
    {showMore && truncated && <span>预览</span>}
    {text && <CopyButton className="content-action" label="复制输出" value={text} onError={onCopyError} />}
  </>}>
    {children || (text ? <div className="tool-output-frame" data-capped={showMore && !expanded || undefined}>
      <div className="tool-output" data-expanded={expanded} tabIndex={0} role="region" aria-label="工具输出">
        <MessageText text={displayed} query="" markdown={markdown} fallback={<div className="tool-output-lines">
          <pre className="tool-line-numbers" aria-hidden="true">{lineNumbers}</pre>
          <pre className="code">{displayed}</pre>
        </div>} />
      </div>
    </div> : <p className="tool-loading tool-payload-message">无输出</p>)}
    {showMore && <button type="button" className="content-action content-expand" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
      {expanded ? "收起输出" : `展开全部 ${lineCount} 行`}
    </button>}
  </ToolSection>;
}
