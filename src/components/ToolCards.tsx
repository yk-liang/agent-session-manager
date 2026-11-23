import { useContext, type ReactNode } from "react";
import {
  Activity, AlarmClock, BookOpenText, Bot, CircleStop, Folder, Globe, Hourglass, Image, ListTodo, MessageCircleQuestionMark,
  Plug, Search, Send, Toolbox, Wrench, type LucideIcon,
} from "lucide-react";
import { compactPath, displayPath, splitPath, type ToolItem } from "../activity";
import { toolDescription } from "../sessionContent";
import type { ContentBlock, ToolOp } from "../types";
import { shortPath } from "../utils";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import { LongText } from "./MessageParts";
import { Chip, OpCard } from "./OpCard";
import { commandView, payloadText, resultText, type View } from "./ToolContent";
import { ToolSections } from "./ToolSections";
import UiIcon from "./UiIcon";

type Op<K extends ToolOp["kind"]> = Extract<ToolOp, { kind: K }>;

function duration(seconds: number) {
  return seconds < 60 ? `${Math.round(seconds * 10) / 10} 秒` : `${Math.round(seconds / 6) / 10} 分钟`;
}

function Title({ children }: { children: string }) {
  return <span className="op-title" title={children}>{children}</span>;
}

function Name({ children, pattern = false }: { children: string; pattern?: boolean }) {
  return <code className={`op-name${pattern ? " pattern" : ""}`} title={children}>{children}</code>;
}

function Detail({ children }: { children?: string }) {
  return children ? <span className="op-detail" title={children}>{children}</span> : null;
}

/** 调用参数里第一个非空的文本字段。 */
function field(call: ContentBlock, keys: string[]) {
  const input = call.input as Record<string, unknown> | undefined;
  return keys.map(key => input?.[key]).find((value): value is string => typeof value === "string" && !!value.trim());
}

function plainView(label: string, text?: string): View | undefined {
  return text ? { label, body: <p className="tool-payload-message tool-plain">{text}</p>, copy: text } : undefined;
}

function longView(label: string, text?: string): View | undefined {
  return text ? { label, body: <LongText text={text} code />, copy: text } : undefined;
}

/** 数据源只保存了密文：如实说明，不把密文当正文展示。 */
function encryptedView(label: string): View {
  return { label, body: <p className="tool-payload-message tool-plain">正文已加密，本地只保存了密文。</p> };
}

/** 技能：标题是技能名和参数，正文是 SKILL.md。 */
export function SkillCard({ item, op }: { item: ToolItem; op: Op<"skill"> }) {
  const split = op.name.lastIndexOf(":") + 1;
  return <OpCard item={item} className="skill" icon={BookOpenText} label="技能" tone="violet"
    head={<>
      <code className="op-name" title={op.name}>{split > 0 && <span className="op-namespace">{op.name.slice(0, split)}</span>}{op.name.slice(split)}</code>
      {op.args && <span className="op-detail code-detail" title={op.args}>{op.args}</span>}
    </>}>
    <SkillBody item={item} />
  </OpCard>;
}

function SkillBody({ item }: { item: ToolItem }) {
  const { markdown, onCopyError } = useContext(DetailState);
  return <ToolSections<Op<"skill">> item={item} input={{ views: op => [plainView("参数", op.args)] }} output={{
    title: "技能说明",
    actions: op => op.path && <CopyButton className="content-action btn-ghost btn-sm" label="复制技能目录" value={op.path} onError={onCopyError}>目录</CopyButton>,
    views: (result, op) => {
      const text = op.body ?? (result && resultText(result));
      if (!text) return [];
      return [{ label: "说明", copy: text, body: <>
        {op.path && <div className="skill-path"><UiIcon icon={Folder} size={12} />{shortPath(op.path)}</div>}
        <div className="skill-doc"><LongText text={text} markdown={markdown} code /></div>
      </> }];
    },
  }} />;
}

type Semantic = {
  icon: LucideIcon;
  label: string;
  head: ReactNode;
  input?: (op: ToolOp, call: ContentBlock) => (View | undefined)[];
  preview?: View;
  output?: string;
};

/** 收件人常是路径、socket 或长 ID：只留末段，长 ID 取前 8 位。 */
function recipient(to: string) {
  const tail = to.split("/").pop() || to;
  return /^[0-9a-f-]{16,}$/i.test(tail) ? tail.slice(0, 8) : tail;
}

function semanticView(op: Exclude<ToolOp, { kind: "read" | "search" | "find" | "list" | "edit" | "write" | "delete" | "shell" | "script" | "skill" }>, cwd: string | null): Semantic {
  switch (op.kind) {
    case "web_search": return {
      icon: Search, label: "网页搜索", output: "结果",
      head: <><Name pattern>{op.query}</Name><span className="op-meta">
        {op.domain && <Chip>{op.domain}</Chip>}{op.results !== undefined && <Chip tone="sans">{op.results} 条结果</Chip>}
      </span></>,
    };
    case "web_fetch": return {
      icon: Globe, label: "读取网页", output: "内容",
      head: <Name>{op.url.replace(/^https?:\/\//, "")}</Name>,
      input: () => [plainView("提问", op.prompt)],
    };
    case "ask": {
      const [first] = op.questions;
      return {
        icon: MessageCircleQuestionMark, label: "向用户提问", output: "回答",
        head: <>
          {first?.header && <Chip tone="kind">{first.header}</Chip>}
          <Title>{first?.question ?? "提问"}</Title>
          <span className="op-meta">
            {op.questions.length > 1 && <Chip tone="sans">共 {op.questions.length} 个问题</Chip>}
            {first?.answer && <Chip tone="answer">{first.answer}</Chip>}
          </span>
        </>,
        preview: { label: "问题", body: op.questions.map((question, i) => <div key={i} className="ask-question">
          <p>{question.question}</p>
          <ul>{question.options.map(option => <li key={option.label} className={option.label === question.answer ? "chosen" : undefined}>
            <strong>{option.label}</strong>{option.description && <span>{option.description}</span>}
          </li>)}</ul>
        </div>) },
      };
    }
    case "agent": return {
      icon: Bot, label: "子代理",
      head: <>{op.agent && <Chip tone="kind">{op.agent}</Chip>}<Title>{op.task}</Title>
        {op.background && <span className="op-meta"><Chip tone="sans">后台</Chip></span>}</>,
      input: (_, call) => [op.encrypted ? encryptedView("任务") : longView("任务", field(call, ["prompt", "Prompt", "message", "task"]))],
    };
    case "message": return {
      icon: Send, label: "发送消息",
      head: op.summary
        ? <><Title>{op.summary}</Title><span className="op-dir" title={op.to}>{`→ ${recipient(op.to)}`}</span></>
        : <><Name>{recipient(op.to)}</Name>{op.encrypted && <Detail>正文已加密</Detail>}</>,
      input: (_, call) => [op.encrypted ? encryptedView("消息") : longView("消息", field(call, ["message", "Message", "content"]))],
    };
    case "wait": return {
      icon: Hourglass, label: "等待",
      head: <><Title>{op.seconds === undefined ? "等待" : `等待 ${duration(op.seconds)}`}</Title>{op.target && <span className="op-meta"><Chip>{op.target}</Chip></span>}</>,
    };
    case "wake": return {
      icon: AlarmClock, label: "定时唤醒",
      head: <><Title>{op.reason ?? "定时唤醒"}</Title><Chip tone="sans">{duration(op.seconds)}</Chip></>,
      input: () => [plainView("说明", op.reason)],
    };
    case "stop": return {
      icon: CircleStop, label: "停止任务",
      head: op.detail ? <><Name>{op.detail}</Name><span className="op-dir" title={op.task}>{op.task}</span></> : <Name>{op.task}</Name>,
    };
    case "task_status": return { icon: Activity, label: "任务状态", head: <Name>{op.task}</Name> };
    case "agent_list": return { icon: Bot, label: "智能体列表", head: <Title>列出子智能体</Title> };
    case "monitor": return {
      icon: Activity, label: "监听后台事件",
      head: <><Title>{op.description}</Title>{op.minutes !== undefined && <span className="op-meta"><Chip tone="sans">{op.minutes} 分钟</Chip></span>}</>,
      input: (_, call) => [commandView(field(call, ["command"]))],
    };
    case "load_tools": return { icon: Toolbox, label: "加载工具", head: <Name>{op.names.join(", ")}</Name> };
    case "todo": {
      const done = op.items.filter(todo => todo.status === "completed").length;
      const current = op.items.find(todo => todo.status === "in_progress");
      return {
        icon: ListTodo, label: "任务清单",
        head: <><Title>{current?.text ?? "更新任务清单"}</Title><span className="op-meta"><Chip tone="sans">{done}/{op.items.length}</Chip></span></>,
        input: full => [{ label: "清单", body: <ul className="todo-list">
          {(full.kind === "todo" ? full.items : op.items).map((todo, i) => <li key={i} data-status={todo.status}>{todo.text}</li>)}
        </ul> }],
      };
    }
    case "image": {
      const { dir, name } = splitPath(displayPath(op.path, cwd));
      return { icon: Image, label: "查看图片", head: <><Name>{name}</Name>{dir && <span className="op-dir">{compactPath(dir)}</span>}</> };
    }
    case "mcp": {
      return {
        icon: Plug, label: "MCP 工具", head: op.title ? <Title>{op.title}</Title> : <Name>{op.server}</Name>,
        input: (_, call) => [argumentView(call.input)],
      };
    }
  }
}

function argumentView(input: unknown): View | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return;
  const entries = Object.entries(input);
  if (!entries.length) return;
  return { label: "参数", body: <dl className="tool-arguments">{entries.map(([key, value]) => <div key={key}>
    <dt>{key}</dt><dd><pre className="code">{payloadText(value)}</pre></dd>
  </div>)}</dl> };
}

/** 网页、提问、子智能体、等待等工具：标题放最有区分度的参数，正文是输入与输出两段。 */
export function SemanticToolCard({ item, op }: { item: ToolItem; op: Parameters<typeof semanticView>[0] }) {
  const { cwd } = useContext(DetailState);
  const view = semanticView(op, cwd);
  return <OpCard item={item} className={op.kind} icon={view.icon} label={view.label} head={view.head}>
    <ToolSections item={item} input={{ preview: view.preview, views: view.input }} output={{ title: view.output }} />
  </OpCard>;
}

/** 没有专门处理的工具：工具名加说明，说明是标题里唯一能看懂的内容。 */
export function GenericToolCard({ item }: { item: ToolItem }) {
  const block = item.part.content;
  const description = toolDescription(block);
  return <OpCard item={item} className="generic" icon={Wrench} label="工具调用"
    head={description ? <Title>{description}</Title> : !block.name && <Name>未知工具</Name>}>
    <ToolSections item={item} />
  </OpCard>;
}
