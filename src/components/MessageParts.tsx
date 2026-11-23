import { useMemo, useState } from "react";
import { matchRanges } from "../conversationText";
import { attachmentLabel } from "../sessionContent";
import type { ContentBlock } from "../types";
import MessageText from "./MessageText";

function attachmentUrl(block: ContentBlock) {
  const source = block.source && typeof block.source === "object" ? block.source as Record<string, unknown> : {};
  const url = source.type === "base64" && typeof source.data === "string"
    ? `data:${source.media_type};base64,${source.data}`
    : block.image_url ?? block.url;
  return url && /^data:image\/(?:png|jpeg|webp|gif);base64,[a-z0-9+/=\s]+$/i.test(url) ? url : undefined;
}

export function Attachment({ block }: { block: ContentBlock }) {
  const label = attachmentLabel(block);
  const src = attachmentUrl(block);
  return src ? <img className="message-image" src={src} alt={label} loading="lazy" /> : <span className="message-attachment">{label}</span>;
}

/** 长文本先显示一段，搜索命中在后文时从命中处附近开始。 */
export function LongText({ text, query = "", code = false, markdown = false }: { text: string; query?: string; code?: boolean; markdown?: boolean }) {
  const [extra, setExtra] = useState(0);
  const [showEarlier, setShowEarlier] = useState(false);
  const initialLimit = 1500;
  const match = useMemo(() => text.length > 3000 && query ? matchRanges(text, query, 1)[0] : undefined, [text, query]);
  const offset = match && match[1] > initialLimit ? Math.max(0, match[0] - 400) : 0;
  const start = showEarlier ? 0 : offset;
  const end = text.length <= 3000 ? text.length : Math.max(offset + initialLimit, match?.[1] ?? 0) + extra;
  const displayed = (start > 0 ? "…" : "") + text.slice(start, end) + (end < text.length ? "…" : "");
  return <>
    {start > 0 && <button type="button" className="content-action btn-ghost btn-sm" onClick={() => setShowEarlier(true)}>显示前文</button>}
    <MessageText text={displayed} query={query} markdown={markdown}
      fallback={code ? <pre className="code">{displayed}</pre> : undefined} />
    {end < text.length && <button type="button" className="content-expand btn-sm"
      onClick={() => setExtra(value => value + 15000)}>显示更多</button>}
  </>;
}
