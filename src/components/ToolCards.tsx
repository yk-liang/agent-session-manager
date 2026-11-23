import { useContext, type ReactNode } from "react";
import {
  Activity, AlarmClock, BookOpenText, Bot, CircleStop, Folder, Globe, Hourglass, Image, ListTodo, MessageCircleQuestionMark,
  Plug, Search, Send, Toolbox, Wrench, type LucideIcon,
} from "lucide-react";
import { compactPath, displayPath, itemStatus, splitPath, type ToolItem } from "../activity";
import { toolDescription } from "../sessionContent";
import type { ToolOp } from "../types";
import { shortPath } from "../utils";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import { LongText } from "./MessageParts";
import { Chip, FullOp, OpCard, ResultOutput, ToolBody, useRawView } from "./OpCard";
import { Payload, resultText, ToolSection } from "./ToolPayload";
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

/** 技能：标题是技能名和参数，正文是 SKILL.md。 */
export function SkillCard({ item, op }: { item: ToolItem; op: Op<"skill"> }) {
  const split = op.name.lastIndexOf(":") + 1;
  return <OpCard item={item} className="skill" icon={BookOpenText} label="技能" tone="violet"
    head={<>
      <code className="op-name" title={op.name}>{split > 0 && <span className="op-namespace">{op.name.slice(0, split)}</span>}{op.name.slice(split)}</code>
      {op.args ? <span className="op-detail code-detail" title={op.args}>{op.args}</span> : <Detail>{op.description}</Detail>}
    </>}>
    <SkillBody item={item} op={op} />
  </OpCard>;
}

/** SKILL.md 本身是 Markdown 文档，随 Markdown 开关渲染。 */
function SkillBody({ item, op }: { item: ToolItem; op: Op<"skill"> }) {
  const { markdown, onCopyError } = useContext(DetailState);
  const { raw, toggle } = useRawView();
  if (raw) return <ToolBody item={item} actions={toggle} />;
  const { result } = item.part;
  const { args, path } = op;
  const actions = <>
    {path && <CopyButton className="content-action btn-ghost btn-sm" label="复制技能目录" value={path} onError={onCopyError}>目录</CopyButton>}
    {toggle}
  </>;
  const document = (text: string) => <ToolSection kind="output" title="技能说明" actions={actions}>
    {path && <div className="skill-path"><UiIcon icon={Folder} size={12} />{shortPath(path)}</div>}
    <div className="skill-doc"><LongText text={text} markdown={markdown} code /></div>
  </ToolSection>;
  return <>
    {args && <ToolSection kind="input" title="参数"><p className="tool-payload-message tool-plain">{args}</p></ToolSection>}
    {result && itemStatus(item) !== "error" ? <FullOp<Op<"skill">> item={item} actions={actions}>{full =>
      full.body !== undefined ? document(full.body) : <Payload block={result} kind="output" actions={actions}>{loaded => document(resultText(loaded))}</Payload>
    }</FullOp> : <ResultOutput item={item} title="技能说明" actions={actions} />}
  </>;
}

type View = { icon: LucideIcon; label: string; head: ReactNode; body?: (actions: ReactNode) => ReactNode };

/** 数据源只保存了密文：如实说明，不把密文当正文展示。 */
function EncryptedSection({ title }: { title: string }) {
  return <ToolSection kind="input" title={title}>
    <p className="tool-payload-message tool-plain">正文已加密，本地只保存了密文。</p>
  </ToolSection>;
}

function semanticView(item: ToolItem, op: Exclude<ToolOp, { kind: "read" | "search" | "find" | "list" | "edit" | "write" | "delete" | "shell" | "script" | "skill" }>, cwd: string | null): View {
  switch (op.kind) {
    case "web_search": return {
      icon: Search, label: "网页搜索",
      head: <><Name pattern>{op.query}</Name><span className="op-meta">
        {op.domain && <Chip>{op.domain}</Chip>}{op.results !== undefined && <Chip tone="sans">{op.results} 条结果</Chip>}
      </span></>,
      body: actions => <ResultOutput item={item} title="结果" actions={actions} />,
    };
    case "web_fetch": return {
      icon: Globe, label: "读取网页",
      head: <><Name>{op.url.replace(/^https?:\/\//, "")}</Name><Detail>{op.prompt}</Detail></>,
      body: actions => <>
        {op.prompt && <ToolSection kind="input" title="提问" actions={actions}><p className="tool-payload-message tool-plain">{op.prompt}</p></ToolSection>}
        <ResultOutput item={item} title="内容" actions={op.prompt ? undefined : actions} />
      </>,
    };
    case "ask": {
      const [first] = op.questions;
      return {
        icon: MessageCircleQuestionMark, label: "向用户提问",
        head: <>
          {first?.header && <Chip tone="kind">{first.header}</Chip>}
          <Title>{first?.question ?? "提问"}</Title>
          <span className="op-meta">
            {op.questions.length > 1 && <Chip tone="sans">共 {op.questions.length} 个问题</Chip>}
            {first?.answer && <Chip tone="answer">{first.answer}</Chip>}
          </span>
        </>,
        body: actions => <>
          <ToolSection kind="output" title="问题" actions={actions}>{op.questions.map((question, i) => <div key={i} className="ask-question">
            <p>{question.question}</p>
            <ul>{question.options.map(option => <li key={option.label} className={option.label === question.answer ? "chosen" : undefined}>
              <strong>{option.label}</strong>{option.description && <span>{option.description}</span>}
            </li>)}</ul>
          </div>)}</ToolSection>
          <ResultOutput item={item} title="回答" />
        </>,
      };
    }
    case "agent": return {
      icon: Bot, label: "子代理",
      head: <>{op.agent && <Chip tone="kind">{op.agent}</Chip>}<Title>{op.task}</Title>
        {op.background && <span className="op-meta"><Chip tone="sans">后台</Chip></span>}</>,
      body: actions => <>
        {op.encrypted ? <EncryptedSection title="任务" /> : <FullOp item={item}>{(_, call) => {
          const input = call.input as Record<string, unknown> | undefined;
          const prompt = ["prompt", "Prompt", "message"].map(key => input?.[key]).find(value => typeof value === "string") as string | undefined;
          return prompt ? <ToolSection kind="input" title="任务"><LongText text={prompt} code /></ToolSection> : null;
        }}</FullOp>}
        <ResultOutput item={item} actions={actions} />
      </>,
    };
    case "message": return {
      icon: Send, label: "发送消息",
      head: <><Name>{op.to}</Name><Detail>{op.encrypted ? "正文已加密" : op.summary}</Detail></>,
      body: op.encrypted ? actions => <><EncryptedSection title="消息" /><ResultOutput item={item} actions={actions} /></> : undefined,
    };
    case "wait": return {
      icon: Hourglass, label: "等待",
      head: <><Title>{op.seconds === undefined ? "等待" : `等待 ${duration(op.seconds)}`}</Title>{op.target && <span className="op-meta"><Chip>{op.target}</Chip></span>}</>,
    };
    case "wake": return { icon: AlarmClock, label: "定时唤醒", head: <><Title>{`${duration(op.seconds)}后唤醒`}</Title><Detail>{op.reason}</Detail></> };
    case "stop": return { icon: CircleStop, label: "停止任务", head: <><Name>{op.task}</Name><Detail>{op.detail}</Detail></> };
    case "monitor": return {
      icon: Activity, label: "监听后台事件",
      head: <><Title>{op.description}</Title>{op.minutes !== undefined && <span className="op-meta"><Chip tone="sans">{op.minutes} 分钟</Chip></span>}</>,
    };
    case "load_tools": return { icon: Toolbox, label: "加载工具", head: <Name>{op.names.join(", ")}</Name> };
    case "todo": {
      const done = op.items.filter(todo => todo.status === "completed").length;
      const current = op.items.find(todo => todo.status === "in_progress");
      return {
        icon: ListTodo, label: "任务清单",
        head: <><Title>{current?.text ?? "更新任务清单"}</Title><span className="op-meta"><Chip tone="sans">{done}/{op.items.length}</Chip></span></>,
        body: actions => <FullOp<Op<"todo">> item={item} actions={actions}>{full => <ToolSection kind="output" title="清单" actions={actions}>
          <ul className="todo-list">{full.items.map((todo, i) => <li key={i} data-status={todo.status}>{todo.text}</li>)}</ul>
        </ToolSection>}</FullOp>,
      };
    }
    case "image": {
      const { dir, name } = splitPath(displayPath(op.path, cwd));
      return { icon: Image, label: "查看图片", head: <><Name>{name}</Name>{dir && <span className="op-dir">{compactPath(dir)}</span>}</> };
    }
    case "mcp": return { icon: Plug, label: "MCP 工具", head: <><Name>{`${op.server} · ${op.tool}`}</Name><Detail>{op.title}</Detail></> };
  }
}

/** 网页、提问、子代理、等待等工具：标题放最有区分度的参数。 */
export function SemanticToolCard({ item, op }: { item: ToolItem; op: Parameters<typeof semanticView>[1] }) {
  const { cwd } = useContext(DetailState);
  const view = semanticView(item, op, cwd);
  return <OpCard item={item} className={op.kind} icon={view.icon} label={view.label} head={view.head}>
    <SemanticBody item={item} body={view.body} />
  </OpCard>;
}

function SemanticBody({ item, body }: { item: ToolItem; body: View["body"] }) {
  const { raw, toggle } = useRawView();
  if (raw) return <ToolBody item={item} actions={toggle} />;
  return body ? body(toggle) : <ResultOutput item={item} actions={toggle} />;
}

/** 没有专门处理的工具：工具名加说明。 */
export function GenericToolCard({ item }: { item: ToolItem }) {
  const block = item.part.content;
  const description = toolDescription(block);
  return <OpCard item={item} className="generic" icon={Wrench} label="工具调用" tag=""
    head={<>
      <code className="collapsible-name" title={block.name}>{block.name || "未知工具"}</code>
      {description && <span className="collapsible-preview" title={description}>{description}</span>}
    </>}>
    <ToolBody item={item} />
  </OpCard>;
}
