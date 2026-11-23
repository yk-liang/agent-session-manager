import { useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import * as api from "../api";
import { attachmentLabel, isAttachment } from "../sessionContent";
import type { ContentBlock } from "../types";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import { Attachment, LongText } from "./MessageParts";
import Notice from "./Notice";

type SectionKind = "input" | "output" | "error";

function payloadText(value: unknown) {
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

/** 延迟加载的工具正文在展开时读取；同一会话内复用已读取的结果。 */
export function Payload({ block, kind, actions, children }: {
  block: ContentBlock;
  kind: SectionKind;
  actions?: ReactNode;
  children: (loaded: ContentBlock) => ReactNode;
}) {
  const { payloads } = useContext(DetailState);
  const token = block.deferred;
  const [loaded, setLoaded] = useState<{ token: string; value?: ContentBlock; error?: string }>();
  const [attempt, setAttempt] = useState(0);
  const current = loaded?.token === token ? loaded : undefined;

  useEffect(() => {
    if (!token) return;
    let cancelled = false;
    const request = payloads.get(token) ?? api.readToolPayload(token);
    payloads.set(token, request);
    void request.then(value => {
      if (!cancelled) setLoaded({ token, value });
    }).catch(error => {
      if (payloads.get(token) === request) payloads.delete(token);
      if (!cancelled) setLoaded({ token, error: String(error) });
    });
    return () => { cancelled = true; };
  }, [token, payloads, attempt]);

  const value = token ? current?.value : block;
  if (value) return children(value);
  return <ToolSection kind={kind} actions={actions}>
    {current?.error ? <Notice tone="error" action={{ label: "重试", onClick: () => {
      setLoaded(undefined);
      setAttempt(count => count + 1);
    } }}>{current.error}</Notice>
      : <div className="tool-loading tool-payload-message" role="status">正在读取…</div>}
  </ToolSection>;
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

function CommandPreview({ command }: { command: string }) {
  return <div className="tool-command"><span aria-hidden="true">$</span><pre className="code"><ShellText command={command} /></pre></div>;
}

/** 命令以 shell 形式展示，可切换到原始参数。 */
export function ToolInput({ value, command, actions }: { value: unknown; command?: string; actions?: ReactNode }) {
  const { onCopyError } = useContext(DetailState);
  const [showRaw, setShowRaw] = useState(false);
  const raw = payloadText(value);
  const showingCommand = !!command && !showRaw;
  return <ToolSection kind="input" title={showingCommand ? "命令" : undefined} actions={<>
    {command && raw && <button type="button" className="content-action btn-ghost btn-sm" onClick={() => setShowRaw(!showRaw)}>
      {showRaw ? "查看命令" : "原始 JSON"}
    </button>}
    {(showingCommand || raw) && <CopyButton className="content-action btn-ghost btn-sm" label={showingCommand ? "复制命令" : "复制输入"}
      value={showingCommand ? command : raw} onError={onCopyError} />}
    {actions}
  </>}>
    {showingCommand ? <CommandPreview command={command} /> : raw
      ? <pre className="code tool-input-code">{raw}</pre>
      : <p className="tool-loading tool-payload-message">无输入</p>}
  </ToolSection>;
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

/** 工具输出始终按原文显示；`start` 为首行行号，为空时不显示行号（命令输出、搜索结果）。 */
export function ToolOutput({ text, title = "输出", start = 1, error = false, actions, children }: {
  text: string;
  title?: string;
  start?: number | null;
  error?: boolean;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  const { onCopyError } = useContext(DetailState);
  const lineCount = useMemo(() => countLines(text), [text]);
  return <ToolSection kind={error ? "error" : "output"} title={error ? "错误输出" : title} actions={<>
    {!children && text && <span>{lineCount} 行</span>}
    {text && <CopyButton className="content-action btn-ghost btn-sm" label={`复制${title}`} value={text} onError={onCopyError} />}
    {actions}
  </>}>
    {children || (text ? <ExpandableFrame lines={lineCount} overflow={text.length > OUTPUT_PREVIEW_LIMIT} label={title}>
      {expanded => {
        const displayed = expanded || text.length <= OUTPUT_PREVIEW_LIMIT ? text : text.slice(0, OUTPUT_PREVIEW_LIMIT);
        const lines = start === null ? undefined : Array.from({ length: countLines(displayed) }, (_, i) => i + start).join("\n");
        return <div className="tool-output-lines" data-numbered={!!lines || undefined}>
          {lines && <pre className="tool-line-numbers" aria-hidden="true">{lines}</pre>}
          <pre className="code">{displayed}</pre>
        </div>;
      }}
    </ExpandableFrame> : <p className="tool-loading tool-payload-message">无{title}</p>)}
  </ToolSection>;
}

type ResultPart = { attachment?: ContentBlock; text: string };

function resultParts(content: unknown[]): ResultPart[] {
  return content.map(part => part && typeof part === "object" && isAttachment(part as ContentBlock)
    ? { attachment: part as ContentBlock, text: attachmentLabel(part as ContentBlock) }
    : { text: typeof (part as ContentBlock)?.text === "string" ? (part as ContentBlock).text! : payloadText(part) });
}

/** 结果的纯文本；附件以名称代替。 */
export function resultText(result: ContentBlock) {
  const content = result.content;
  if (!Array.isArray(content)) return content == null ? "" : payloadText(content);
  return resultParts(content).map(part => part.text).join("\n\n");
}

/** 结果可以是文本，也可以混有图片等附件。 */
export function ToolResult({ result, failed = !!result.is_error, title, start, actions }: {
  result: ContentBlock;
  failed?: boolean;
  title?: string;
  start?: number | null;
  actions?: ReactNode;
}) {
  const parts = Array.isArray(result.content) ? resultParts(result.content) : [];
  return <ToolOutput text={resultText(result)} title={title} start={start} error={failed} actions={actions}>
    {parts.some(part => part.attachment) && <div className="tool-result-parts">{parts.map((part, index) => part.attachment
      ? <Attachment key={index} block={part.attachment} />
      : <LongText key={index} text={part.text} code />)}</div>}
  </ToolOutput>;
}
