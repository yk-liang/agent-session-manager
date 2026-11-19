import { createContext, memo, useContext, useEffect, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Layers, type LucideIcon } from "lucide-react";
import * as api from "../api";
import type { ContentBlock } from "../types";
import { attachmentLabel, hasActivity, isAttachment, messageBlockText, type SessionRow, type SessionBlock, type Visibility } from "../sessionContent";
import { absoluteTime } from "../utils";
import CopyButton from "./CopyButton";
import MessageText from "./MessageText";
import UiIcon from "./UiIcon";

function attachmentUrl(block: ContentBlock) {
  const source = block.source && typeof block.source === "object" ? block.source as Record<string, unknown> : {};
  const url = source.type === "base64" && typeof source.data === "string"
    ? `data:${source.media_type};base64,${source.data}`
    : block.image_url ?? block.url;
  return url && /^data:image\/(?:png|jpeg|webp|gif);base64,[a-z0-9+/=\s]+$/i.test(url) ? url : undefined;
}

function Attachment({ block }: { block: ContentBlock }) {
  const label = attachmentLabel(block);
  const src = attachmentUrl(block);
  return src ? <img className="message-image" src={src} alt={label} loading="lazy" /> : <span className="message-attachment">{label}</span>;
}

function ToolResult({ result, markdown }: { result: ContentBlock; markdown: boolean }) {
  const content = result.content;
  if (content == null || content === "" || Array.isArray(content) && !content.length) {
    return <p className="tool-loading">暂无结果内容</p>;
  }
  if (!Array.isArray(content)) {
    const text = typeof content === "string" ? content : JSON.stringify(content, null, 2);
    return <LongText text={text} code markdown={markdown} />;
  }
  return <div className="tool-result-parts">{content.map((part, index) => {
    if (part && typeof part === "object" && isAttachment(part)) return <Attachment key={index} block={part} />;
    const text = typeof part?.text === "string" ? part.text : JSON.stringify(part, null, 2);
    return <LongText key={index} text={text ?? ""} code markdown={markdown} />;
  })}</div>;
}

// 虚拟行移出视口后会卸载，展开状态和正在读取的内容仍属于本次会话。
export const DetailState = createContext({ expanded: new Set<string>(), payloads: new Map<string, Promise<ContentBlock>>(), markdown: false });
export const RowKey = createContext("");
function LongText({ text, query = "", code = false, markdown = false }: { text: string; query?: string; code?: boolean; markdown?: boolean }) {
  const [limit, setLimit] = useState(1500);
  const long = text.length > 3000;
  const complete = !long || limit >= text.length;
  const hit = query ? text.toLocaleLowerCase().indexOf(query.toLocaleLowerCase()) : -1;
  const start = !complete && hit >= limit ? Math.max(0, hit - 400) : 0;
  const visible = complete ? text : text.slice(start, start + limit);
  const displayed = (start > 0 ? "…" : "") + visible + (!complete ? "…" : "");
  return <><MessageText text={displayed} query={query} markdown={markdown}
    fallback={code ? <pre className="code">{displayed}</pre> : undefined} />
    {!complete && <button className="expand-text" onClick={() => setLimit(Math.max(limit + 15000, start + limit + 15000))}>显示更多</button>}
  </>;
}
function ToolContent({ block, input = false }: { block: ContentBlock; input?: boolean }) {
  const state = useContext(DetailState);
  const [loaded, setLoaded] = useState<ContentBlock | null>(block.deferred ? null : block);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let cancelled = false;
    if (!block.deferred) { setLoaded(block); return; }
    const token = block.deferred;
    let request = state.payloads.get(token);
    if (!request) { request = api.readToolPayload(token); state.payloads.set(token, request); }
    void request.then((value) => { if (!cancelled) setLoaded(value); }).catch((e) => {
      state.payloads.delete(token);
      if (!cancelled) setError(String(e));
    });
    return () => { cancelled = true; };
  }, [block, state, attempt]);
  if (error) return <div role="alert">{error} <button onClick={() => { setError(""); setAttempt((n) => n + 1); }}>重试</button></div>;
  if (!loaded) return <div className="tool-loading" role="status">加载中…</div>;
  if (!input) return <ToolResult result={loaded} markdown={state.markdown} />;
  const text = typeof loaded.input === "string" ? loaded.input : JSON.stringify(loaded.input, null, 2) ?? "";
  return <LongText text={text} code />;
}

function Collapsible({
  label,
  name,
  summary,
  preview,
  trailing,
  icon,
  children,
  error,
  kind,
  persistKey,
}: {
  label: string;
  name?: string;
  summary?: string | null;
  preview?: string;
  trailing?: ReactNode;
  icon?: LucideIcon;
  children: ReactNode;
  error?: boolean;
  kind: "thinking" | "tool" | "context" | "response-group";
  persistKey: string;
}) {
  const { expanded } = useContext(DetailState);
  const rowKey = useContext(RowKey);
  const key = `${rowKey}:${persistKey}`;
  const [open, setOpen] = useState(() => expanded.has(key));
  const description = summary ?? (open ? undefined : preview);
  const toggle = () => { if (open) expanded.delete(key); else expanded.add(key); setOpen(!open); };
  return (
    <div className={`collapsible ${kind}${error ? " error" : ""}`}>
      <button className="collapsible-head" aria-expanded={open} onClick={toggle}>
        <UiIcon icon={open ? ChevronDown : ChevronRight} className="caret" />
        {icon && <UiIcon icon={icon} className="response-group-icon" />}
        <span className="collapsible-label">{label}</span>
        {name && <code className="collapsible-name" title={name}>{name}</code>}
        {description && <span className="collapsible-preview" title={description}>{description}</span>}
        {trailing && <span className="collapsible-status">{trailing}</span>}
      </button>
      {open && <div className="collapsible-body">{children}</div>}
    </div>
  );
}

const toolStatusLabels = { pending: "等待中", running: "进行中", completed: "已完成", error: "失败" };

function toolStatus(block: ContentBlock, result?: ContentBlock) {
  return block.status ?? result?.status ?? (result && (result.is_error ? "error" : "completed"));
}

function toolDescription(block: ContentBlock) {
  const input = block.input && typeof block.input === "object" ? block.input as Record<string, unknown> : {};
  const description = [block.description, input.description, input.toolSummary, input.query]
    .find((value): value is string => typeof value === "string" && !!value.trim());
  return description?.trim().replace(/\s+/g, " ");
}

function ToolCard({ block, result, index }: { block: ContentBlock; result?: ContentBlock; index: number }) {
  const status = toolStatus(block, result);
  return (
    <Collapsible
      kind="tool"
      error={status === "error"}
      persistKey={`block:${index}`}
      label="工具调用"
      name={block.name || "未知工具"}
      preview={toolDescription(block)}
      trailing={status && <span className={`tool-status ${status}`}>{toolStatusLabels[status]}</span>}
    >
      <div className="tool-section-title">输入</div>
      <ToolContent key={block.deferred} block={block} input />
      {result && (
        <>
          <div className="tool-section-title">结果</div>
          <ToolContent key={result.deferred} block={result} />
        </>
      )}
    </Collapsible>
  );
}

function ActivityCard({ part, markdown }: { part: SessionBlock; markdown: boolean }) {
  const { content: block, index, result } = part;
  if (block.type === "thinking") {
    return <Collapsible persistKey={`block:${index}`} kind="thinking" label="思考过程"
      preview={block.thinking?.trim().replace(/\s+/g, " ").slice(0, 240)}>
      <LongText text={block.thinking ?? ""} code markdown={markdown} />
    </Collapsible>;
  }
  if (block.type === "tool_result") {
    return <Collapsible persistKey={`block:${index}`} kind="tool" label="工具结果"
      error={block.is_error} trailing={block.is_error && <span className="tool-status error">失败</span>}>
      <ToolContent key={block.deferred} block={block} />
    </Collapsible>;
  }
  return <ToolCard block={block} index={index} result={result} />;
}

function ResponseGroup({ parts, visibility, markdown }: {
  parts: SessionBlock[];
  visibility: Visibility;
  markdown: boolean;
}) {
  const visible = parts.filter(part => visibility[hasActivity(part)!]);
  if (!visible.length) return null;
  const rows = visible.map(part => <ActivityCard key={part.index} part={part} markdown={markdown} />);
  if (parts.length === 1) return rows[0];
  const thinkingCount = visible.filter(({ content }) => content.type === "thinking").length;
  const toolCount = visible.filter(({ content }) => content.type === "tool_use").length;
  const resultCount = visible.length - thinkingCount - toolCount;
  const failureCount = visible.filter(({ content, result }) => content.type === "tool_result"
    ? content.is_error : content.type === "tool_use" && toolStatus(content, result) === "error").length;
  const label = thinkingCount && toolCount + resultCount ? "思考与工具调用"
    : thinkingCount ? "思考过程" : toolCount ? "工具调用" : "工具结果";
  const counts = [
    [thinkingCount, "段思考"],
    [toolCount, "次调用"],
    [resultCount, "条结果"],
    [failureCount, "次失败"],
  ] as const;
  return <Collapsible kind="response-group" persistKey={`group:${parts[0].index}`}
    label={label} trailing={counts.map(([count, unit]) => count > 0 && <span key={unit}
      className={unit === "次失败" ? "collapsible-count failed" : "collapsible-count"}><strong>{count}</strong> {unit}</span>)}
    error={failureCount > 0} icon={Layers}>{rows}</Collapsible>;
}

function MessageBubble({ role, timestamp, blocks, copyText, markdown, query, onCopyError }: {
  role: "user" | "assistant";
  timestamp?: string;
  blocks: SessionBlock[];
  copyText: string;
  markdown: boolean;
  query: string;
  onCopyError: (error: unknown) => void;
}) {
  const time = timestamp && absoluteTime(timestamp);
  return <div className={`message ${role}`}>
    <div className={`msg ${role}`}>{blocks.map(({ index, content }) => content.type === "text"
      ? <div key={index} className="msg-text"><LongText text={content.text?.trim() ?? ""} query={query} markdown={markdown} /></div>
      : <Attachment key={index} block={content} />)}</div>
    <div className="message-actions">
      {time && <time dateTime={timestamp}>{time}</time>}
      <CopyButton className="message-copy" value={copyText} label="复制消息" onError={onCopyError} />
    </div>
  </div>;
}

export const EventView = memo(function EventView({ row, visibility, renderMarkdown, highlightQuery = "", onCopyError }: {
  row: SessionRow;
  visibility: Visibility;
  renderMarkdown: boolean;
  highlightQuery?: string;
  onCopyError: (error: unknown) => void;
}) {
  const { event, blocks } = row;
  const { type, timestamp } = event;
  if (type === "context") {
    const text = event.message?.content;
    if (typeof text !== "string" || !text.trim()) return null;
    return <Collapsible kind="context" persistKey="context" label={event.context?.label ?? "运行上下文"}
      summary={event.context?.summary}><LongText text={text} code /></Collapsible>;
  }
  if (type === "system") {
    const text = String(event.content ?? event.subtype ?? "").trim();
    return text ? <div className="system-line">{text}</div> : null;
  }

  const parts: ReactNode[] = [];
  let activity: SessionBlock[] = [];
  let message: SessionBlock[] = [];
  const copyText = blocks.flatMap(({ content }) => messageBlockText(content) ?? []).join("\n\n");
  const flushActivity = () => {
    if (!activity.length) return;
    parts.push(<ResponseGroup key={activity[0].index} parts={activity} visibility={visibility} markdown={renderMarkdown} />);
    activity = [];
  };
  const flushMessage = () => {
    if (!message.length) return;
    parts.push(<MessageBubble key={message[0].index} role={type} timestamp={timestamp} blocks={message}
      copyText={copyText} markdown={renderMarkdown} query={highlightQuery} onCopyError={onCopyError} />);
    message = [];
  };

  for (const part of blocks) {
    if (hasActivity(part)) {
      flushMessage();
      activity.push(part);
      continue;
    }
    if (!messageBlockText(part.content)?.trim()) continue;
    flushActivity();
    message.push(part);
  }
  flushActivity();
  flushMessage();
  return <>{parts}</>;
});
