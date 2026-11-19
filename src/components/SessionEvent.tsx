import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Layers, type LucideIcon } from "lucide-react";
import * as api from "../api";
import type { ContentBlock } from "../types";
import { hasActivity, isAttachment, type SessionRow, type SessionBlock, type Visibility } from "../sessionContent";
import MessageText from "./MessageText";
import UiIcon from "./UiIcon";

function attachmentLabel(block: ContentBlock) {
  return block.filename?.trim() || (block.type === "image" || block.type === "input_image" ? "图片" : "附件");
}

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
  preview,
  trailing,
  icon,
  children,
  error,
  kind,
  persistKey,
}: {
  label: string;
  preview?: string;
  trailing?: string;
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
  const toggle = () => { if (open) expanded.delete(key); else expanded.add(key); setOpen(!open); };
  return (
    <div className={`collapsible ${kind}${error ? " error" : ""}`}>
      <button className="collapsible-head" aria-expanded={open} onClick={toggle}>
        <UiIcon icon={open ? ChevronDown : ChevronRight} className="caret" />
        {icon && <UiIcon icon={icon} className="response-group-icon" />}
        <span className="collapsible-label" title={kind === "tool" ? label : undefined}>{label}</span>
        {!open && preview && <span className="collapsible-preview" title={preview}>{preview}</span>}
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
      label={`工具调用 · ${block.name ?? "未知工具"}`}
      preview={toolDescription(block)}
      trailing={status && toolStatusLabels[status]}
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
      error={block.is_error} trailing={block.is_error ? "失败" : undefined}>
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
    thinkingCount ? `${thinkingCount} 段思考` : "",
    toolCount ? `${toolCount} 次调用` : "",
    resultCount ? `${resultCount} 条结果` : "",
    failureCount ? `${failureCount} 次失败` : "",
  ].filter(Boolean).join(" · ");
  return <Collapsible kind="response-group" persistKey={`group:${parts[0].index}`}
    label={label} trailing={counts} error={failureCount > 0} icon={Layers}>{rows}</Collapsible>;
}

export function EventView({ row, visibility, renderMarkdown, highlightQuery = "" }: {
  row: SessionRow;
  visibility: Visibility;
  renderMarkdown: boolean;
  highlightQuery?: string;
}) {
  const { event, blocks } = row;
  if (event.type === "context") {
    const text = event.message?.content;
    if (typeof text !== "string" || !text.trim()) return null;
    const label = [event.context?.label ?? "运行上下文", event.context?.summary].filter(Boolean).join(" · ");
    return <Collapsible kind="context" persistKey="context" label={label}><LongText text={text} code /></Collapsible>;
  }
  if (event.type === "system") {
    const text = String(event.content ?? event.subtype ?? "").trim();
    return text ? <div className="system-line">{text}</div> : null;
  }

  const parts: ReactNode[] = [];
  let activity: SessionBlock[] = [];
  let message: ReactNode[] = [];
  let messageStart = 0;
  const flushActivity = () => {
    if (!activity.length) return;
    parts.push(<ResponseGroup key={activity[0].index} parts={activity} visibility={visibility} markdown={renderMarkdown} />);
    activity = [];
  };
  const flushMessage = () => {
    if (!message.length) return;
    parts.push(<div key={messageStart} className={`msg ${event.type}`}>{message}</div>);
    message = [];
  };

  for (const part of blocks) {
    const { index, content } = part;
    if (hasActivity(part)) {
      flushMessage();
      activity.push(part);
      continue;
    }
    const text = content.type === "text" && content.text?.trim();
    const attachment = isAttachment(content);
    if (!text && !attachment) continue;
    flushActivity();
    if (!message.length) messageStart = index;
    message.push(text
      ? <div key={index} className="msg-text"><LongText text={text} query={highlightQuery} markdown={renderMarkdown} /></div>
      : <Attachment key={index} block={content} />);
  }
  flushActivity();
  flushMessage();
  return <>{parts}</>;
}
