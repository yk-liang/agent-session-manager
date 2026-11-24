import { useContext } from "react";
import {
  Activity, AlarmClock, BookOpenText, Bot, CircleStop, Folder, Hourglass, Image, ListTodo, MessageCircleQuestionMark,
  Plug, Send, SquareTerminal, Toolbox, Wrench,
} from "lucide-react";
import { type ToolItem } from "../activity";
import { toolDescription } from "../sessionContent";
import type { ContentBlock, ToolOp } from "../types";
import { displayPath, shortPath } from "../utils";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import MessageText from "./MessageText";
import { Chip, OpCard, PathName, type Head } from "./OpCard";
import { resultText } from "./ResultViews";
import { commandView, ExpandableFrame, payloadText, plainView, TextPreview, textView, type View } from "./ToolContent";
import { ToolSections } from "./ToolSections";
import UiIcon from "./UiIcon";

type Op<K extends ToolOp["kind"]> = Extract<ToolOp, { kind: K }>;

function duration(seconds: number) {
  return seconds < 60 ? `${Math.round(seconds * 10) / 10} 秒` : `${Math.round(seconds / 6) / 10} 分钟`;
}

function Title({ children }: { children: string }) {
  return <span className="op-title" title={children}>{children}</span>;
}

function Name({ children, title = children }: { children: string; title?: string }) {
  return <code className="op-name" title={title}>{children}</code>;
}

function Detail({ children }: { children?: string }) {
  return children ? <span className="op-detail" title={children}>{children}</span> : null;
}

/** 调用参数里第一个非空的文本字段。 */
function field(call: ContentBlock, keys: string[]) {
  const input = call.input as Record<string, unknown> | undefined;
  return keys.map(key => input?.[key]).find((value): value is string => typeof value === "string" && !!value.trim());
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
    <SkillBody item={item} op={op} />
  </OpCard>;
}

function SkillBody({ item, op }: { item: ToolItem; op: Op<"skill"> }) {
  const { markdown, onCopyError } = useContext(DetailState);
  const read = ["read", "read_file"].includes(item.part.content.name?.toLowerCase() ?? "");
  const path = read && (field(item.part.content, ["path", "file_path", "filePath"]) ?? (op.path && `${op.path}/SKILL.md`));
  return <ToolSections<Op<"skill">> item={item} input={{
    preview: path ? textView("路径", path, null) : undefined,
    loadOnExpand: true,
    views: (op, call) => {
      const path = field(call, ["path", "file_path", "filePath"]);
      return [path ? textView("路径", path, null) : commandView(field(call, ["command", "cmd", "CommandLine"])) ?? plainView("参数", op.args)
        ?? plainView("技能", field(call, ["skill", "name", "id"]))];
    },
  }} output={{
    title: "技能说明",
    actions: op => op.path && <CopyButton className="content-action btn-ghost btn-sm" label="复制技能目录" value={op.path} onError={onCopyError}>目录</CopyButton>,
    views: (result, op) => {
      const text = op.body ?? (result && resultText(result));
      if (!text) return [];
      return [{ label: "说明", copy: text, body: <TextPreview text={text} label="说明">{text => <>
        {op.path && <div className="skill-path" title={op.path}><UiIcon icon={Folder} size={12} />{shortPath(op.path)}</div>}
        <div className="skill-doc"><MessageText text={text} query="" markdown={markdown} fallback={<pre className="code">{text}</pre>} /></div>
      </>}</TextPreview> }];
    },
  }} />;
}

type Semantic = Head & {
  summary: string;
  input?: (op: ToolOp, call: ContentBlock) => (View | undefined)[];
  preview?: View;
  output?: string;
};

const terminalActions = { poll: "读取后台输出", send: "发送会话输入", interrupt: "中断会话" } as const;

/** 收件人常是路径、socket 或长 ID：只留末段，长 ID 取前 8 位。 */
function recipient(to: string) {
  const tail = to.split("/").pop() || to;
  return /^[0-9a-f-]{16,}$/i.test(tail) ? tail.slice(0, 8) : tail;
}

const NON_SEMANTIC = new Set(["read", "search", "find", "list", "edit", "write", "delete", "shell", "script", "skill", "exec", "web_search", "web_fetch"]);
export type SemanticOp = Exclude<ToolOp, { kind: "read" | "search" | "find" | "list" | "edit" | "write" | "delete" | "shell" | "script" | "skill" | "exec" | "web_search" | "web_fetch" }>;

export function isSemanticOp(op: ToolOp): op is SemanticOp {
  return !NON_SEMANTIC.has(op.kind);
}

export function semanticView(op: SemanticOp, cwd: string | null, name?: string): Semantic {
  switch (op.kind) {
    case "ask": {
      const [first] = op.questions;
      return {
        icon: MessageCircleQuestionMark, label: "向用户提问", output: "回答",
        summary: first?.question ?? "",
        head: <>
          {first?.header && <Chip tone="kind">{first.header}</Chip>}
          <Title>{first?.question ?? "提问"}</Title>
          <span className="op-meta">
            {op.questions.length > 1 && <Chip tone="sans">共 {op.questions.length} 个问题</Chip>}
            {first?.answer && <Chip tone="answer">{first.answer}</Chip>}
          </span>
        </>,
        preview: { label: "问题", body: <ExpandableFrame label="问题">{() => op.questions.map((question, i) => <div key={i} className="ask-question">
          <p>{question.question}</p>
          <ul>{question.options.map(option => <li key={option.label} className={option.label === question.answer ? "chosen" : undefined}>
            <strong>{option.label}</strong>{option.description && <span>{option.description}</span>}
          </li>)}</ul>
        </div>)}</ExpandableFrame> },
      };
    }
    case "agent": return {
      icon: Bot, label: "子智能体", summary: op.task,
      head: <>{op.agent && <Chip tone="kind">{op.agent}</Chip>}<Title>{op.task}</Title>
        {op.background && <span className="op-meta"><Chip tone="sans">后台</Chip></span>}</>,
      input: (_, call) => [op.encrypted ? encryptedView("任务") : plainView("任务", field(call, ["prompt", "Prompt", "message", "task"]), true)],
    };
    case "message": return {
      icon: Send, label: name === "followup_task" ? "追派任务" : name === "SubagentHandback" ? "交回任务" : "发送消息",
      summary: op.summary ? `${op.summary} → ${recipient(op.to)}` : recipient(op.to),
      head: op.summary
        ? <><Title>{op.summary}</Title><span className="op-dir" title={op.to}>{`→ ${recipient(op.to)}`}</span></>
        : <><Name title={op.to}>{recipient(op.to)}</Name>{op.encrypted && <Detail>正文已加密</Detail>}</>,
      input: (_, call) => [op.encrypted ? encryptedView("消息") : plainView("消息", field(call, ["message", "Message", "content"]), true)],
    };
    case "wait": {
      const text = [op.target, op.seconds !== undefined && duration(op.seconds)].filter(Boolean).join(" · ");
      return {
        icon: Hourglass, label: "等待", summary: text,
        head: <><Title>{op.seconds === undefined ? "等待" : `等待 ${duration(op.seconds)}`}</Title>{op.target && <span className="op-meta"><Chip>{op.target}</Chip></span>}</>,
        preview: plainView("等待条件", [op.target, op.seconds !== undefined && duration(op.seconds)].filter(Boolean).join("\n")),
      };
    }
    case "wake": return {
      icon: AlarmClock, label: "定时唤醒", summary: op.reason ?? duration(op.seconds),
      head: <><Title>{op.reason ?? "定时唤醒"}</Title><Chip tone="sans">{duration(op.seconds)}</Chip></>,
      preview: plainView("说明", op.reason),
    };
    case "stop": return {
      icon: CircleStop, label: name === "interrupt_agent" ? "中断子智能体" : name === "close_agent" ? "结束子智能体" : "停止任务",
      summary: op.detail ?? op.task,
      head: op.detail ? <><Name>{op.detail}</Name><span className="op-dir" title={op.task}>{op.task}</span></> : <Name>{op.task}</Name>,
    };
    case "task_status": return { icon: Activity, label: "任务状态", summary: op.task, head: <Name>{op.task}</Name> };
    case "terminal_input": return {
      icon: SquareTerminal, label: terminalActions[op.action], summary: `会话 ${op.session}`, head: <Name>{`会话 ${op.session}`}</Name>,
      input: (_, call) => [plainView("标准输入", field(call, ["chars"]), true) ?? plainView("会话", String(op.session))],
    };
    case "agent_list": return { icon: Bot, label: "智能体列表", summary: "列出子智能体", head: <Title>列出子智能体</Title> };
    case "monitor": return {
      icon: Activity, label: "监听后台事件", summary: op.description,
      head: <><Title>{op.description}</Title>{op.minutes !== undefined && <span className="op-meta"><Chip tone="sans">{op.minutes} 分钟</Chip></span>}</>,
      input: (_, call) => [commandView(field(call, ["command"]))],
    };
    case "load_tools": return { icon: Toolbox, label: "加载工具", summary: op.names.join(", "), head: <Name>{op.names.join(", ")}</Name> };
    case "todo": {
      const done = op.items.filter(todo => todo.status === "completed").length;
      const current = op.items.find(todo => todo.status === "in_progress");
      return {
        icon: ListTodo, label: "任务清单", summary: current?.text ?? `${op.items.length} 项`,
        head: <><Title>{current?.text ?? "更新任务清单"}</Title><span className="op-meta"><Chip tone="sans">{done}/{op.items.length}</Chip></span></>,
        preview: { label: "清单", body: <ExpandableFrame label="清单">{() => <ul className="todo-list">
          {op.items.map((todo, i) => <li key={i} data-status={todo.status}>{todo.text}</li>)}
        </ul>}</ExpandableFrame> },
      };
    }
    case "image": return {
      icon: Image, label: "查看图片", summary: displayPath(op.path, cwd),
      head: <PathName path={op.path} cwd={cwd} />,
      preview: textView("路径", op.path, null),
    };
    case "generate_image": return {
      icon: Image, label: "生成图片", summary: op.path ? displayPath(op.path, cwd) : "生成图片",
      head: op.path ? <PathName path={op.path} cwd={cwd} /> : null,
      input: (_, call) => [plainView("提示词", field(call, ["revisedPrompt", "prompt"]))],
    };
    case "report": return {
      icon: Activity, label: op.title, summary: op.summary ?? op.title, head: op.summary ? <Title>{op.summary}</Title> : null, output: "结果",
      input: (_, call) => [plainView("说明", op.summary) ?? argumentView(call.input)],
    };
    case "mcp": return {
      icon: Plug, label: "MCP 工具", summary: [op.tool, op.title].filter(Boolean).join(" · "),
      head: <><Name title={`${op.server} / ${op.tool}`}>{op.tool}</Name>{op.title ? <Detail>{op.title}</Detail> : <span className="op-dir">{op.server}</span>}</>,
      input: (_, call) => [argumentView(call.input)],
    };
  }
}

function argumentView(input: unknown): View | undefined {
  if (!input || typeof input !== "object" || Array.isArray(input)) return;
  const entries = Object.entries(input);
  if (!entries.length) return;
  return { label: "参数", body: <ExpandableFrame label="参数">{() => <dl className="tool-arguments">{entries.map(([key, value]) => <div key={key}>
    <dt>{key}</dt><dd><pre className="code">{payloadText(value)}</pre></dd>
  </div>)}</dl>}</ExpandableFrame> };
}

/** 提问、子智能体、等待、MCP 等工具：标题放最有区分度的参数，正文是输入与输出两段。 */
export function SemanticToolCard({ item, op }: { item: ToolItem; op: SemanticOp }) {
  const { cwd } = useContext(DetailState);
  const view = semanticView(op, cwd, item.part.content.name);
  return <OpCard item={item} className={op.kind} icon={view.icon} label={view.label} head={view.head}>
    <ToolSections item={item} input={{ preview: view.preview, views: view.input }}
      output={{ title: view.output, terminal: op.kind === "terminal_input" }} />
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
