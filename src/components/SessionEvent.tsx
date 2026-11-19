import { createContext, memo, useContext, useEffect, useState, type ReactNode } from "react";
import { Brain, ChevronDown, ChevronRight, FileOutput, FileText, Wrench, type LucideIcon } from "lucide-react";
import * as api from "../api";
import type { ContentBlock } from "../types";
import { attachmentLabel, isAttachment, messageBlockText, visibleBlockKind, type SessionRow, type SessionBlock, type Visibility } from "../sessionContent";
import { formatMessageTimestamp } from "../utils";
import CopyButton from "./CopyButton";
import MessageText from "./MessageText";
import { payloadText, ToolInput, ToolOutput, ToolSection } from "./ToolPayload";
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

function ToolResult({ result, markdown, failed, onCopyError }: {
  result: ContentBlock;
  markdown: boolean;
  failed: boolean;
  onCopyError: (error: unknown) => void;
}) {
  const content = result.content;
  if (!Array.isArray(content)) {
    return <ToolOutput text={content == null ? "" : payloadText(content)} markdown={markdown} error={failed} onCopyError={onCopyError} />;
  }
  const parts = content.map(part => {
    if (part && typeof part === "object" && isAttachment(part)) return { attachment: part, text: attachmentLabel(part) };
    return { text: typeof part?.text === "string" ? part.text : payloadText(part) };
  });
  return <ToolOutput text={parts.map(part => part.text).join("\n\n")} markdown={markdown} error={failed} onCopyError={onCopyError}>
    {parts.some(part => part.attachment) && <div className="tool-result-parts">{parts.map((part, index) => part.attachment
      ? <Attachment key={index} block={part.attachment} />
      : <LongText key={index} text={part.text} code markdown={markdown} />)}</div>}
  </ToolOutput>;
}

export const DetailState = createContext({
  expanded: new Set<string>(),
  payloads: new Map<string, Promise<ContentBlock>>(),
  markdown: false,
  onCopyError: (_error: unknown) => {},
});
export const RowKey = createContext("");
function LongText({ text, query = "", code = false, markdown = false }: { text: string; query?: string; code?: boolean; markdown?: boolean }) {
  const [limit, setLimit] = useState(1500);
  const long = text.length > 3000;
  const complete = !long || limit >= text.length;
  const matchOffset = query ? text.toLocaleLowerCase().indexOf(query.toLocaleLowerCase()) : -1;
  const start = !complete && matchOffset >= limit ? Math.max(0, matchOffset - 400) : 0;
  const visible = complete ? text : text.slice(start, start + limit);
  const displayed = (start > 0 ? "…" : "") + visible + (!complete ? "…" : "");
  return <><MessageText text={displayed} query={query} markdown={markdown}
    fallback={code ? <pre className="code">{displayed}</pre> : undefined} />
    {!complete && <button type="button" className="content-action content-expand"
      onClick={() => setLimit(Math.max(limit + 15000, start + limit + 15000))}>显示更多</button>}
  </>;
}
function ToolContent({ block, input = false, failed = false }: { block: ContentBlock; input?: boolean; failed?: boolean }) {
  const { payloads, markdown, onCopyError } = useContext(DetailState);
  const token = block.deferred;
  const [loadedPayload, setLoadedPayload] = useState<{ token: string; value?: ContentBlock; error?: string }>();
  const [attempt, setAttempt] = useState(0);
  const result = loadedPayload?.token === token ? loadedPayload : undefined;
  const loaded = token ? result?.value : block;
  const error = token ? result?.error : undefined;

  useEffect(() => {
    if (!token) return;
    let cancelled = false;
    const request = payloads.get(token) ?? api.readToolPayload(token);
    payloads.set(token, request);
    void request.then(value => {
      if (!cancelled) setLoadedPayload({ token, value });
    }).catch(error => {
      if (payloads.get(token) === request) payloads.delete(token);
      if (!cancelled) setLoadedPayload({ token, error: String(error) });
    });
    return () => { cancelled = true; };
  }, [token, payloads, attempt]);
  if (error || !loaded) return <ToolSection kind={input ? "input" : failed ? "error" : "output"}>
    {error ? <div className="tool-payload-message" role="alert">
      {error} <button type="button" onClick={() => { setLoadedPayload(undefined); setAttempt(value => value + 1); }}>重试</button>
    </div> : <div className="tool-loading tool-payload-message" role="status">正在读取…</div>}
  </ToolSection>;
  if (input) return <ToolInput value={loaded.input} onCopyError={onCopyError} />;
  return <ToolResult result={loaded} markdown={markdown} failed={failed || !!loaded.is_error} onCopyError={onCopyError} />;
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
  icon: LucideIcon;
  children: ReactNode;
  error?: boolean;
  kind: "thinking" | "tool" | "context";
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
      <button type="button" className="collapsible-head" aria-expanded={open} onClick={toggle}>
        <UiIcon icon={open ? ChevronDown : ChevronRight} className="caret" />
        <span className="collapsible-type" title={label}>
          <UiIcon icon={icon} size={16} />
          <span className="sr-only">{label}</span>
        </span>
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
  if (block.status === "error" || result?.status === "error" || result?.is_error) return "error";
  if (block.status === "completed" || result?.status === "completed") return "completed";
  return result?.status ?? block.status ?? (result && "completed");
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
      icon={Wrench}
      name={block.name || "未知工具"}
      summary={toolDescription(block)}
      trailing={status && <span className={`tool-status ${status}`}>{toolStatusLabels[status]}</span>}
    >
      <ToolContent key={block.deferred ? `input:${block.deferred}` : "input"} block={block} input />
      {result && <ToolContent key={result.deferred ? `output:${result.deferred}` : "output"} block={result} failed={status === "error"} />}
    </Collapsible>
  );
}

function ActivityCard({ part, markdown }: { part: SessionBlock; markdown: boolean }) {
  const { content: block, index, result } = part;
  if (block.type === "thinking") {
    return <Collapsible persistKey={`block:${index}`} kind="thinking" label="思考过程" icon={Brain}
      preview={block.thinking?.trim().replace(/\s+/g, " ").slice(0, 240)}>
      <LongText text={block.thinking ?? ""} code markdown={markdown} />
    </Collapsible>;
  }
  if (block.type === "tool_result") {
    return <Collapsible persistKey={`block:${index}`} kind="tool" label="工具输出" icon={FileOutput}
      error={block.is_error} trailing={block.is_error && <span className="tool-status error">失败</span>}>
      <ToolContent key={block.deferred} block={block} failed={block.is_error} />
    </Collapsible>;
  }
  return <ToolCard block={block} index={index} result={result} />;
}

function ResponseGroup({ parts, markdown }: {
  parts: SessionBlock[];
  markdown: boolean;
}) {
  const rows = parts.map(part => <ActivityCard key={part.index} part={part} markdown={markdown} />);
  if (rows.length === 1) return rows[0];
  const failed = parts.some(({ content, result }) => content.type === "tool_result"
    ? content.is_error : content.type === "tool_use" && toolStatus(content, result) === "error");
  return <div className={`response-group${failed ? " error" : ""}`}>{rows}</div>;
}

function MessageBubble({ role, timestamp, blocks, copyText, markdown, query }: {
  role: "user" | "assistant";
  timestamp?: string;
  blocks: SessionBlock[];
  copyText: string;
  markdown: boolean;
  query: string;
}) {
  const { onCopyError } = useContext(DetailState);
  const time = timestamp && formatMessageTimestamp(timestamp);
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

export const EventView = memo(function EventView({ row, visibility, renderMarkdown, highlightQuery = "" }: {
  row: SessionRow;
  visibility: Visibility;
  renderMarkdown: boolean;
  highlightQuery?: string;
}) {
  const { event, blocks } = row;
  const { type, timestamp } = event;
  if (type === "context") {
    const text = event.message?.content;
    if (typeof text !== "string" || !text.trim()) return null;
    return <Collapsible kind="context" persistKey="context" label={event.context?.label ?? "运行上下文"} icon={FileText}
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
    parts.push(<ResponseGroup key={activity[0].index} parts={activity} markdown={renderMarkdown} />);
    activity = [];
  };
  const flushMessage = () => {
    if (!message.length) return;
    parts.push(<MessageBubble key={message[0].index} role={type} timestamp={timestamp} blocks={message}
      copyText={copyText} markdown={renderMarkdown} query={highlightQuery} />);
    message = [];
  };

  for (const part of blocks) {
    const kind = visibleBlockKind(part, visibility);
    if (!kind) continue;
    if (kind === "activity") {
      flushMessage();
      activity.push(part);
      continue;
    }
    flushActivity();
    message.push(part);
  }
  flushActivity();
  flushMessage();
  return <>{parts}</>;
});
