import { memo, useContext, type ReactNode } from "react";
import { FileText } from "lucide-react";
import { messageBlockText, visibleBlockKind, type SessionBlock, type SessionRow, type Visibility } from "../sessionContent";
import { formatMessageTimestamp } from "../utils";
import { ActivityList } from "./ActivityCards";
import Collapsible from "./Collapsible";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import { Attachment, LongText } from "./MessageParts";
import UiIcon from "./UiIcon";

function MessageBubble({ role, timestamp, blocks, markdown, query }: {
  role: "user" | "assistant";
  timestamp?: string;
  blocks: SessionBlock[];
  markdown: boolean;
  query: string;
}) {
  const { onCopyError } = useContext(DetailState);
  const time = timestamp && formatMessageTimestamp(timestamp);
  const copyText = blocks.flatMap(({ content }) => messageBlockText(content) ?? []).join("\n\n");
  return <div className={`message ${role}`}>
    <div className={`msg ${role}`}>{blocks.map(({ key, content }) => content.type === "text"
      ? <div key={key} className="msg-text"><LongText text={content.text?.trim() ?? ""} query={query} markdown={markdown} /></div>
      : <Attachment key={key} block={content} />)}</div>
    <div className="message-actions">
      {time && <time dateTime={timestamp}>{time}</time>}
      <CopyButton className="message-copy btn-icon btn-ghost" value={copyText} label="复制消息" onError={onCopyError} />
    </div>
  </div>;
}

/** 首行之后的事件只含工具和思考，与首行末尾的活动连成一段渲染。 */
export const EventView = memo(function EventView({ index, rows, visibility, renderMarkdown, highlightQuery = "" }: {
  index: number;
  rows: SessionRow[];
  visibility: Visibility;
  renderMarkdown: boolean;
  highlightQuery?: string;
}) {
  const [{ event }] = rows;
  if (event.type === "context") {
    const text = event.message?.content;
    if (typeof text !== "string" || !text.trim()) return null;
    const label = event.context?.label ?? "运行上下文";
    const summary = event.context?.summary?.trim();
    const preview = text.trim().slice(0, 320).replace(/\s+/g, " ").slice(0, 240);
    return <Collapsible id={`${index}.context`} className="context" head={<>
      <span className="collapsible-type" title={label}><UiIcon icon={FileText} size={16} /><span>{label}</span></span>
      {summary ? <span className="collapsible-preview" title={summary}>{summary}</span>
        : <span className="collapsible-preview closed-only" title={preview} aria-hidden="true">{preview}</span>}
    </>}>
      <LongText text={text} code />
    </Collapsible>;
  }
  if (event.type === "system") {
    const text = String(event.content ?? event.subtype ?? "").trim();
    return text ? <div className="system-line">{text}</div> : null;
  }

  const parts: ReactNode[] = [];
  let activity: SessionBlock[] = [];
  const flushActivity = () => {
    if (!activity.length) return;
    parts.push(<ActivityList key={activity[0].key} parts={activity} />);
    activity = [];
  };
  for (const { event, blocks } of rows) {
    let message: SessionBlock[] = [];
    const flushMessage = () => {
      if (!message.length) return;
      parts.push(<MessageBubble key={message[0].key} role={event.type as "user" | "assistant"} timestamp={event.timestamp}
        blocks={message} markdown={renderMarkdown} query={highlightQuery} />);
      message = [];
    };
    for (const part of blocks) {
      const kind = visibleBlockKind(part, visibility);
      if (kind === "activity") {
        flushMessage();
        activity.push(part);
      } else if (kind === "message") {
        flushActivity();
        message.push(part);
      }
    }
    flushMessage();
  }
  flushActivity();
  return <>{parts}</>;
});
