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

const COMMAND_FIELDS = ["command", "cmd", "commandline"] as const;
const DESCRIPTION_FIELDS = ["description", "toolsummary", "query"] as const;

function toolInputValues(input: unknown, fields: readonly string[]) {
  if (!input || typeof input !== "object" || Array.isArray(input)) return [];
  const entries = Object.entries(input);
  return fields.flatMap(name =>
    entries.filter(([key]) => key.replace(/[_-]/g, "").toLowerCase() === name).map(([, value]) => value),
  );
}

export function toolStatus(block: ContentBlock, result?: ContentBlock) {
  if (block.status === "error" || result?.status === "error" || result?.is_error) return "error";
  if (block.status === "completed" || result?.status === "completed") return "completed";
  return result?.status ?? block.status ?? (result && "completed");
}

export function toolDescription(block: ContentBlock) {
  const description = [block.description, ...toolInputValues(block.input, DESCRIPTION_FIELDS)]
    .find((value): value is string => typeof value === "string" && !!value.trim());
  return description?.trim().replace(/\s+/g, " ");
}

export function toolCommand(input: unknown): string | undefined {
  for (const command of toolInputValues(input, COMMAND_FIELDS)) {
    if (typeof command === "string" && command.trim()) return command;
    if (Array.isArray(command) && command.length && command.every(arg => typeof arg === "string")) {
      return command.map(arg => /^[\w./:=@%+,\-]+$/.test(arg) ? arg : "'" + arg.replace(/'/g, "'\\''") + "'").join(" ");
    }
  }
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

export function visibleBlockKind(part: SessionBlock, visibility: Visibility) {
  const activity = hasActivity(part);
  if (activity) return visibility[activity] ? "activity" : undefined;
  if (messageBlockText(part.content)?.trim()) return "message";
}

export function rowVisible({ event, blocks }: SessionRow, visibility: Visibility) {
  if (event.type === "context") return visibility.context && typeof event.message?.content === "string" && !!event.message.content.trim();
  if (event.type === "system") return !!String(event.content ?? event.subtype ?? "").trim();
  return blocks.some(part => visibleBlockKind(part, visibility));
}

export function rowEndsWithMessage({ blocks }: SessionRow, visibility: Visibility) {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const kind = visibleBlockKind(blocks[i], visibility);
    if (kind) return kind === "message";
  }
  return false;
}

/** 展开状态和已读取的工具正文；虚拟行卸载后仍保留，同一会话的正文刷新也沿用。 */
export type SessionReading = { expanded: Set<string>; payloads: Map<string, Promise<ContentBlock>> };

function createSessionContent(events: SessionEvent[]) {
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
  };
}

const sessionContents = new WeakMap<SessionEvent[], ReturnType<typeof createSessionContent>>();

export function sessionContent(events: SessionEvent[]) {
  let content = sessionContents.get(events);
  if (!content) {
    content = createSessionContent(events);
    sessionContents.set(events, content);
  }
  return content;
}

export function visibleSessionRows(rows: SessionRow[], visibility: Visibility) {
  const indexes: number[] = [];
  rows.forEach((row, index) => { if (rowVisible(row, visibility)) indexes.push(index); });
  return indexes;
}
