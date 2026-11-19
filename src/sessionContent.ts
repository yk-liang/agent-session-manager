import type { ContentBlock, SessionEvent } from "./types";

export type Activity = "thinking" | "tool";
export type Visibility = { tool: boolean; thinking: boolean; context: boolean };
export const DEFAULT_VISIBILITY: Visibility = { tool: true, thinking: true, context: false };
export type SessionBlock = {
  index: number;
  content: ContentBlock;
  result?: ContentBlock;
  linked?: boolean;
};
export type SessionRow = { event: SessionEvent; blocks: SessionBlock[] };

function blocks(content: unknown): SessionBlock[] {
  if (typeof content === "string") return [{ index: 0, content: { type: "text", text: content } }];
  if (!Array.isArray(content)) return [];
  return content.flatMap((raw, index) => {
    if (raw === null || typeof raw !== "object" || typeof raw.type !== "string") return [];
    const block = raw as Record<string, unknown>;
    const string = (key: string) => typeof block[key] === "string" ? block[key] as string : undefined;
    const status = block.status;
    return [{ index, content: {
      type: block.type as string,
      status: status === "pending" || status === "running" || status === "completed" || status === "error" ? status : undefined,
      deferred: string("deferred"), text: string("text"), thinking: string("thinking"),
      name: string("name"), description: string("description"), id: string("id"),
      tool_use_id: string("tool_use_id"), input: block.input, content: block.content,
      is_error: typeof block.is_error === "boolean" ? block.is_error : undefined,
      source: block.source, url: string("url"), image_url: string("image_url"), filename: string("filename"),
    } }];
  });
}

export function sessionRows(events: SessionEvent[]): SessionRow[] {
  const calls = new Map<string, SessionBlock>();
  return events.map(event => {
    const content = event.type === "assistant" || event.type === "user" && !event.isMeta
      ? blocks(event.message?.content) : [];
    for (const part of content) {
      const block = part.content;
      if (block.type === "tool_use" && block.id) calls.set(block.id, part);
      if (block.type === "tool_result" && block.tool_use_id) {
        const call = calls.get(block.tool_use_id);
        if (!call) continue;
        call.result = block;
        part.linked = true;
        calls.delete(block.tool_use_id);
      }
    }
    return { event, blocks: content };
  });
}

export function hasActivity(part: SessionBlock): Activity | undefined {
  if (part.content.type === "thinking" && part.content.thinking?.trim()) return "thinking";
  if (part.content.type === "tool_use" || part.content.type === "tool_result" && !part.linked) return "tool";
}

export function isAttachment(block: ContentBlock) {
  return ["image", "input_image", "input_file", "file", "document"].includes(block.type);
}

export function attachmentLabel(block: ContentBlock) {
  return block.filename?.trim() || (block.type === "image" || block.type === "input_image" ? "图片" : "附件");
}

export function messageBlockText(block: ContentBlock) {
  if (block.type === "text") return block.text;
  if (isAttachment(block)) return attachmentLabel(block);
}

export function rowVisible({ event, blocks }: SessionRow, visibility: Visibility) {
  if (event.type === "context") return visibility.context && typeof event.message?.content === "string" && !!event.message.content.trim();
  if (event.type === "system") return !!String(event.content ?? event.subtype ?? "").trim();
  return blocks.some(part => {
    const activity = hasActivity(part);
    return activity ? visibility[activity] : !!messageBlockText(part.content)?.trim();
  });
}

/** 展开状态和已读取的工具正文；虚拟行卸载后仍保留，同一会话的正文刷新也沿用。 */
export type SessionReading = { expanded: Set<string>; payloads: Map<string, Promise<ContentBlock>> };

function createSessionContent(events: SessionEvent[], reading?: SessionReading) {
  const rows = sessionRows(events);
  const toc: { idx: number; text: string }[] = [];
  let hasContext = false;
  rows.forEach(({ event, blocks }, idx) => {
    if (event.type === "context") hasContext = true;
    if (event.type !== "user" || event.isMeta) return;
    const text = blocks.flatMap(({ content }) => messageBlockText(content)?.trim() || []).join("\n");
    if (text) toc.push({ idx, text });
  });
  return {
    rows, toc, hasContext,
    visible: visibleSessionRows(rows, DEFAULT_VISIBILITY),
    reading: reading ?? { expanded: new Set<string>(), payloads: new Map<string, Promise<ContentBlock>>() },
  };
}

const sessionContents = new WeakMap<SessionEvent[], ReturnType<typeof createSessionContent>>();

/** `reading` 只在首次为这份正文建立视图时采用，用于同一会话重读后接续阅读状态。 */
export function sessionContent(events: SessionEvent[], reading?: SessionReading) {
  let content = sessionContents.get(events);
  if (!content) {
    content = createSessionContent(events, reading);
    sessionContents.set(events, content);
  }
  return content;
}

export function visibleSessionRows(rows: SessionRow[], visibility: Visibility) {
  const indexes: number[] = [];
  rows.forEach((row, index) => { if (rowVisible(row, visibility)) indexes.push(index); });
  return indexes;
}
