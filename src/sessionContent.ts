import type { AgentEnvelope, ContentBlock, SessionEvent, ToolOp } from "./types";

export type Activity = "thinking" | "tool";
export type Visibility = { tool: boolean; thinking: boolean; context: boolean };
export const DEFAULT_VISIBILITY: Visibility = { tool: true, thinking: true, context: false };
export type SessionBlock = {
  /** 会话内唯一，用于保存展开状态。 */
  key: string;
  content: ContentBlock;
  result?: ContentBlock;
  linked?: boolean;
};
export type SessionRow = { event: SessionEvent; blocks: SessionBlock[] };
/** 可见行由首个事件和紧随其后、只有工具或思考的事件组成，连续的活动在同一行内渲染。 */
export type VisibleRow = { index: number; members: number[] };

function blocks(content: unknown, row: number): SessionBlock[] {
  if (typeof content === "string") return [{ key: `${row}.0`, content: { type: "text", text: content } }];
  if (!Array.isArray(content)) return [];
  return content.flatMap((raw, index) => {
    if (raw === null || typeof raw !== "object" || typeof raw.type !== "string") return [];
    const block = raw as Record<string, unknown>;
    const string = (key: string) => typeof block[key] === "string" ? block[key] as string : undefined;
    const status = block.status;
    return [{ key: `${row}.${index}`, content: {
      type: block.type as string,
      status: status === "pending" || status === "running" || status === "completed" || status === "error" ? status : undefined,
      deferred: string("deferred"), text: string("text"), thinking: string("thinking"),
      name: string("name"), description: string("description"), id: string("id"),
      tool_use_id: string("tool_use_id"), input: block.input, content: block.content,
      is_error: typeof block.is_error === "boolean" ? block.is_error : undefined,
      error_summary: string("error_summary"),
      command_output_kind: block.command_output_kind === "empty" || block.command_output_kind === "diff" ? block.command_output_kind : undefined,
      command_output: string("command_output"),
      exit_code: typeof block.exit_code === "number" ? block.exit_code : undefined,
      script_result: typeof block.script_result === "object" && block.script_result !== null ? block.script_result as ContentBlock : undefined,
      source: block.source, url: string("url"), image_url: string("image_url"), uri: string("uri"), filename: string("filename"),
      ops: Array.isArray(block.ops) ? block.ops as ToolOp[] : undefined,
    } }];
  });
}

/** 对话消息：用户、助手和其他智能体发来的消息，不含自动注入的元消息。 */
export function isMessageEvent(event: SessionEvent) {
  return event.type === "assistant" || event.type === "agent" || event.type === "user" && !event.isMeta;
}

/** 工具调用与结果只在同一段内配对，子会话自己的结果不会并入继承的调用。 */
export function sessionRows(events: SessionEvent[], inherited = 0): SessionRow[] {
  const calls = new Map<string, SessionBlock>();
  return events.map((event, row) => {
    if (row === inherited) calls.clear();
    const content = isMessageEvent(event) ? blocks(event.message?.content, row) : [];
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

function hasActivity(part: SessionBlock): Activity | undefined {
  if (part.content.type === "thinking" && part.content.thinking?.trim()) return "thinking";
  if (part.content.type === "tool_use" || part.content.type === "tool_result" && !part.linked) return "tool";
}

const DESCRIPTION_FIELDS = ["description", "toolsummary", "query", "completionsummary", "message", "action"] as const;

export function toolStatus(block: ContentBlock, result?: ContentBlock) {
  if (block.status === "error" || result?.status === "error" || result?.is_error) return "error";
  if (block.status === "completed" || result?.status === "completed") return "completed";
  return result?.status ?? block.status ?? (result && "completed");
}

export function toolDescription(block: ContentBlock) {
  const input = block.input && typeof block.input === "object" && !Array.isArray(block.input) ? Object.entries(block.input) : [];
  const description = [block.description, ...DESCRIPTION_FIELDS.flatMap(name =>
    input.filter(([key]) => key.replace(/[_-]/g, "").toLowerCase() === name).map(([, value]) => value))]
    .find((value): value is string => typeof value === "string" && !!value.trim());
  return description?.trim().replace(/\s+/g, " ");
}

export function isAttachment(block: ContentBlock) {
  return ["image", "input_image", "input_file", "file", "document"].includes(block.type);
}

export function attachmentLabel(block: ContentBlock) {
  return block.filename?.trim() || block.name?.trim() || (block.type === "image" || block.type === "input_image" ? "图片" : "附件");
}

export function messageBlockText(block: ContentBlock) {
  if (block.type === "text") return block.text;
  if (isAttachment(block)) return attachmentLabel(block);
}

export const AGENT_KINDS = { task: "任务", message: "消息", result: "结果" } as const;

/** 上级智能体派来的任务和追加消息，在导览中和用户提问同等对待。 */
function fromAncestor({ from, to }: AgentEnvelope) {
  return !!from && to.startsWith(`${from}/`);
}

export function agentTaskName({ to }: AgentEnvelope) {
  return to.slice(to.lastIndexOf("/") + 1);
}

/** 正文加密时，导览用类型和来源代替正文。 */
function agentSummary(agent: AgentEnvelope) {
  return agent.kind === "task" ? `${AGENT_KINDS.task} ${agentTaskName(agent)}` : `${agent.from} 的${AGENT_KINDS[agent.kind]}`;
}

export function visibleBlockKind(part: SessionBlock, visibility: Visibility) {
  const activity = hasActivity(part);
  if (activity) return visibility[activity] ? "activity" : undefined;
  if (messageBlockText(part.content)?.trim()) return "message";
}

export function rowVisible({ event, blocks }: SessionRow, visibility: Visibility) {
  if (event.type === "agent") return true;
  if (event.type === "context") return visibility.context && typeof event.message?.content === "string" && !!event.message.content.trim();
  if (event.type === "system") return !!String(event.content ?? event.subtype ?? "").trim();
  return blocks.some(part => visibleBlockKind(part, visibility));
}

function lastKind({ event, blocks }: SessionRow, visibility: Visibility) {
  if (event.type === "agent") return "message";
  for (let i = blocks.length - 1; i >= 0; i--) {
    const kind = visibleBlockKind(blocks[i], visibility);
    if (kind) return kind;
  }
}

/** 一个虚拟行最多合并的事件数，避免长串工具调用绕过列表虚拟化。 */
const MAX_MERGED_ROWS = 40;

export function visibleSessionRows(rows: SessionRow[], visibility: Visibility, from = 0, to = rows.length): VisibleRow[] {
  const visible: VisibleRow[] = [];
  let openActivity = false;
  for (let index = from; index < to; index++) {
    const row = rows[index];
    if (!rowVisible(row, visibility)) continue;
    const onlyActivity = row.blocks.every(part => visibleBlockKind(part, visibility) !== "message");
    const open = visible[visible.length - 1];
    if (openActivity && onlyActivity && row.blocks.length && open.members.length < MAX_MERGED_ROWS) open.members.push(index);
    else visible.push({ index, members: [] });
    openActivity = lastKind(row, visibility) === "activity";
  }
  return visible;
}

/** 子会话开头继承自父会话的事件单独成段，活动不跨段合并。 */
export function visibleSegments(rows: SessionRow[], visibility: Visibility, inherited: number) {
  return {
    inherited: visibleSessionRows(rows, visibility, 0, inherited),
    own: visibleSessionRows(rows, visibility, inherited),
  };
}

export function rowEndsWithMessage(rows: SessionRow[], { index, members }: VisibleRow, visibility: Visibility) {
  return lastKind(rows[members.length ? members[members.length - 1] : index], visibility) === "message";
}

/** 展开状态和已读取的工具正文；虚拟行卸载后仍保留，同一会话的正文刷新也沿用。 */
export type SessionReading = { expanded: Map<string, boolean>; payloads: Map<string, Promise<ContentBlock>> };

function createSessionContent(events: SessionEvent[], inherited: number) {
  const rows = sessionRows(events, inherited);
  const toc: { idx: number; text: string }[] = [];
  let hasContext = false;
  let inheritedMessages = 0;
  rows.forEach(({ event, blocks }, idx) => {
    if (event.type === "context") hasContext = true;
    const text = blocks.flatMap(({ content }) => messageBlockText(content)?.trim() || []).join("\n");
    const { agent } = event;
    if (idx < inherited && (text || agent)) inheritedMessages++;
    const prompt = agent ? fromAncestor(agent) && (text || agentSummary(agent)) : event.type === "user" && text;
    if (prompt) toc.push({ idx, text: prompt });
  });
  return {
    rows, toc, hasContext, inherited, inheritedMessages,
    visible: visibleSegments(rows, DEFAULT_VISIBILITY, inherited),
  };
}

const sessionContents = new WeakMap<SessionEvent[], ReturnType<typeof createSessionContent>>();

/** `inherited` 是开头继承自父会话的事件数；同一份事件只按首次传入的值切分。 */
export function sessionContent(events: SessionEvent[], inherited = 0) {
  let content = sessionContents.get(events);
  if (!content) {
    content = createSessionContent(events, Math.min(inherited, events.length));
    sessionContents.set(events, content);
  }
  return content;
}
