import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import CopyButton from "./CopyButton";
import UiIcon from "./UiIcon";
import ViewToggle from "./ViewToggle";
import { useVirtualizer } from "@tanstack/react-virtual";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import ConfirmDialog from "./ConfirmDialog";
import SessionInfo from "./SessionInfo";
import MessageText from "./MessageText";
import { readMarkdownPreference, saveMarkdownPreference } from "../markdownPreference";
import { eventMatches, eventTextParts } from "../conversationText";
import type { AgentInfo, ContentBlock, ConversationSearchTarget, SessionDetail as Detail, SessionEvent } from "../types";
import { fullTime, sessionKey } from "../utils";
import { historicalModels, eventModelTitle } from "../sessionModels";
import * as api from "../api";

interface Props {
  detail: Detail;
  agent: AgentInfo;
  onDeleted: () => void;
  onDeletingChange: (deleting: boolean) => void;
  toast: (msg: string, isError?: boolean) => void;
  searchTarget?: ConversationSearchTarget | null;
}

function contentBlocks(content: unknown): ContentBlock[] {
  if (!Array.isArray(content)) return [];
  return content.flatMap((raw): ContentBlock[] => {
    if (raw === null || typeof raw !== "object") return [];
    const block = raw as Record<string, unknown>;
    if (typeof block.type !== "string") return [];
    return [{
      type: block.type,
      status: block.status === "pending" || block.status === "running" || block.status === "completed" || block.status === "error" ? block.status : undefined,
      deferred: typeof block.deferred === "string" ? block.deferred : undefined,
      text: typeof block.text === "string" ? block.text : undefined,
      thinking: typeof block.thinking === "string" ? block.thinking : undefined,
      name: typeof block.name === "string" ? block.name : undefined,
      id: typeof block.id === "string" ? block.id : undefined,
      input: block.input,
      tool_use_id: typeof block.tool_use_id === "string" ? block.tool_use_id : undefined,
      content: block.content,
      is_error: typeof block.is_error === "boolean" ? block.is_error : undefined,
    }];
  });
}

function extractPrompt(ev: SessionEvent): string {
  const c = ev.message?.content;
  if (typeof c === "string") return c.trim();
  const text = contentBlocks(c)
    .filter((b) => b.type === "text" && b.text)
    .map((b) => b.text)
    .join("\n");
  return text.trim();
}

function resultText(b: ContentBlock): string {
  if (typeof b.content === "string") return b.content;
  if (Array.isArray(b.content)) {
    return b.content
      .map((x: { type?: string; text?: string }) =>
        x && x.type === "text" ? x.text ?? "" : JSON.stringify(x),
      )
      .join("\n");
  }
  return JSON.stringify(b.content ?? "");
}

// 虚拟行移出视口后会卸载，展开状态和正在读取的内容仍属于本次会话。
const DetailState = createContext({ expanded: new Set<string>(), payloads: new Map<string, Promise<ContentBlock>>(), markdown: false });
const RowKey = createContext("");
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
    {!complete && <button className="expand-text" onClick={() => setLimit(Math.max(limit + 15000, start + limit + 15000))}>展开更多</button>}
  </>;
}
function ToolContent({ block, input = false, onResolved }: { block: ContentBlock; input?: boolean; onResolved?: (block: ContentBlock) => void }) {
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
    void request.then((value) => { if (!cancelled) { setLoaded(value); onResolved?.(value); } }).catch((e) => {
      state.payloads.delete(token);
      if (!cancelled) setError(String(e));
    });
    return () => { cancelled = true; };
  }, [block, state, attempt, onResolved]);
  if (error) return <div role="alert">{error} <button onClick={() => { setError(""); setAttempt((n) => n + 1); }}>重试</button></div>;
  if (!loaded) return <div className="tool-loading" role="status">加载中…</div>;
  // 仅展开后序列化，折叠状态不创建格式化后的大字符串。
  const text = input ? JSON.stringify(loaded.input, null, 2) ?? "" : resultText(loaded);
  return <LongText text={text} code markdown={!input && state.markdown} />;
}

function Collapsible({
  label,
  preview,
  children,
  error,
  kind,
  persistKey,
}: {
  label: string;
  preview?: string;
  children: React.ReactNode;
  error?: boolean;
  kind?: "thinking" | "tool" | "context";
  persistKey?: string;
}) {
  const state = useContext(DetailState);
  const rowKey = useContext(RowKey);
  const key = rowKey + ":" + (persistKey ?? label) + ":" + (preview ?? "");
  const [open, setOpen] = useState(() => state.expanded.has(key));
  const toggle = () => { if (open) state.expanded.delete(key); else state.expanded.add(key); setOpen(!open); };
  return (
    <div className={`collapsible ${error ? "error" : ""} ${kind ?? ""}`}>
      <button className="collapsible-head" aria-expanded={open} onClick={toggle}>
        <UiIcon icon={open ? ChevronDown : ChevronRight} className="caret" />
        <span className="collapsible-label">{label}</span>
        {!open && preview && <span className="collapsible-preview">{preview}</span>}
      </button>
      {open && <div className="collapsible-body">{children}</div>}
    </div>
  );
}

function ToolCard({ b, result }: { b: ContentBlock; result?: ContentBlock }) {
  const [resolved, setResolved] = useState<ContentBlock | undefined>(result);
  const status = result?.deferred ? resolved : result;
  const stateLabel = b.status ? { pending: "等待中", running: "进行中", completed: "已完成", error: "失败" }[b.status]
    : status && !status.deferred ? (status.is_error ? "失败" : "已完成") : "";
  return (
    <Collapsible
      kind="tool"
      error={b.status === "error" || status?.is_error}
      persistKey={`tool:${b.id ?? b.name}`}
      label={`工具调用 · ${b.name ?? "?"}${stateLabel ? ` · ${stateLabel}` : ""}`}
    >
      <div className="tool-section-title">输入</div>
      <ToolContent block={b} input />
      {result && (
        <>
          <div className="tool-section-title">结果</div>
          <ToolContent block={result} onResolved={setResolved} />
        </>
      )}
    </Collapsible>
  );
}

function EventView({
  ev,
  resultsByUseId,
  showTools,
  showThinking,
  renderMarkdown,
  highlightQuery = "",
}: {
  ev: SessionEvent;
  resultsByUseId: Map<string, ContentBlock>;
  showTools: boolean;
  showThinking: boolean;
  renderMarkdown: boolean;
  highlightQuery?: string;
}) {
  if (ev.type === "context") {
    const text = ev.message?.content;
    if (typeof text !== "string" || !text.trim()) return null;
    const label = [ev.context?.label ?? "运行上下文", ev.context?.summary].filter(Boolean).join(" · ");
    return <Collapsible kind="context" label={label}>
      <LongText text={text} code />
    </Collapsible>;
  }
  if (ev.type === "system") {
    const text = String(ev.content ?? ev.subtype ?? "").trim();
    if (!text) return null;
    return <div className="system-line">{text}</div>;
  }

  if (ev.type === "user") {
    if (ev.isMeta) return null;
    const c = ev.message?.content;
    const textParts: React.ReactNode[] = [];
    const resultParts: React.ReactNode[] = [];

    const pushText = (raw: string, key: string) => {
      const clean = raw.trim();
      if (clean) textParts.push(<div className="msg-text" key={key}><LongText text={clean} query={highlightQuery} markdown={renderMarkdown} /></div>);
    };

    if (typeof c === "string") {
      pushText(c, "s");
    } else {
      const blocks = contentBlocks(c);
      blocks.forEach((b, i) => {
        if (b.type === "text" && b.text) pushText(b.text, String(i));
      });
      blocks.forEach((b, i) => {
        if (
          showTools &&
          b.type === "tool_result" &&
          (!b.tool_use_id || !resultsByUseId.has(b.tool_use_id))
        ) {
          resultParts.push(
            <Collapsible
              key={`tr-${i}`}
              kind="tool"
              error={b.is_error}
              label={b.is_error ? "工具结果 · 失败" : "工具结果"}
            >
              <ToolContent block={b} />
            </Collapsible>,
          );
        }
      });
    }
    if (textParts.length === 0 && resultParts.length === 0) return null;
    return (
      <>
        {textParts.length > 0 && <div className="msg user">{textParts}</div>}
        {resultParts}
      </>
    );
  }

  // assistant
  const raw = ev.message?.content;
  const c: ContentBlock[] =
    typeof raw === "string" ? [{ type: "text", text: raw }] : contentBlocks(raw);
  if (c.length === 0) return null;
  return (
    <>
      {c.map((b, i) => {
        if (b.type === "thinking" && b.thinking) {
          if (!showThinking) return null;
          const preview = b.thinking.replace(/\s+/g, " ").slice(0, 60);
          return (
            <Collapsible key={i} kind="thinking" label="思考过程" preview={preview + "…"}>
              <LongText text={b.thinking} code markdown={renderMarkdown} />
            </Collapsible>
          );
        }
        if (b.type === "text" && b.text) {
          const clean = b.text.trim();
          if (!clean) return null;
          return (
            <div key={i} className="msg assistant">
              <div className="msg-text"><LongText text={clean} query={highlightQuery} markdown={renderMarkdown} /></div>
            </div>
          );
        }
        if (b.type === "tool_use") {
          if (!showTools) return null;
          const result = b.id ? resultsByUseId.get(b.id) : undefined;
          return <ToolCard key={b.id ?? i} b={b} result={result} />;
        }
        return null;
      })}
    </>
  );
}

function eventVisible(ev: SessionEvent, tools: boolean, thinking: boolean, context: boolean, results: Map<string, ContentBlock>) {
  if (ev.type === "context") return context && typeof ev.message?.content === "string" && !!ev.message.content.trim();
  if (ev.type === "system") return !!String(ev.content ?? ev.subtype ?? "").trim();
  if (ev.type === "user" && ev.isMeta) return false;
  const content = ev.message?.content;
  if (typeof content === "string") return !!content.trim();
  return contentBlocks(content).some((b) =>
    (b.type === "text" && !!b.text && !!b.text.trim()) ||
    (ev.type === "assistant" && ((b.type === "tool_use" && tools) || (b.type === "thinking" && thinking && !!b.thinking))) ||
    (ev.type === "user" && tools && b.type === "tool_result" && (!b.tool_use_id || !results.has(b.tool_use_id)))
  );
}

export default function SessionDetailView({ detail, agent, onDeleted, onDeletingChange, toast, searchTarget = null }: Props) {
  const { meta } = detail;
  const toTrash = agent.deletePolicy === "trash";
  const removing = useRef(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [showTools, setShowTools] = useState(true);
  const [showThinking, setShowThinking] = useState(true);
  const [showContext, setShowContext] = useState(false);
  const [renderMarkdown, setRenderMarkdown] = useState(readMarkdownPreference);

  const resultsByUseId = useMemo(() => {
    const useIds = new Set<string>();
    for (const ev of detail.events) {
      if (ev.type !== "assistant") continue;
      for (const block of contentBlocks(ev.message?.content)) {
        if (block.type === "tool_use" && block.id) useIds.add(block.id);
      }
    }

    const map = new Map<string, ContentBlock>();
    for (const ev of detail.events) {
      if (ev.type !== "user") continue;
      for (const block of contentBlocks(ev.message?.content)) {
        if (
          block.type === "tool_result" &&
          block.tool_use_id &&
          useIds.has(block.tool_use_id)
        ) {
          map.set(block.tool_use_id, block);
        }
      }
    }
    return map;
  }, [detail]);

  const toc = useMemo(() => {
    const items: { idx: number; text: string }[] = [];
    detail.events.forEach((ev, i) => {
      if (ev.type !== "user" || ev.isMeta) return;
      const text = extractPrompt(ev);
      if (text) items.push({ idx: i, text });
    });
    return items;
  }, [detail]);

  const models = useMemo(() => agent.id === "opencode" ? historicalModels(detail.events) : [], [agent.id, detail]);
  const bodyRef = useRef<HTMLDivElement>(null);
  const hitIndex = useMemo(() => {
    if (!searchTarget) return -1;
    const matches = (index: number) => index >= 0 && !!detail.events[index] && eventMatches(detail.events[index], searchTarget.query);
    const byId = searchTarget.eventId ? detail.events.findIndex((event) => event.uuid === searchTarget.eventId) : -1;
    if (matches(byId)) return byId;
    const anchor = searchTarget.snippet.replace(/^…|…$/g, "");
    const anchored = (index: number) => matches(index) && eventTextParts(detail.events[index]).some((text) => text.includes(anchor));
    if (anchored(searchTarget.eventIndex)) return searchTarget.eventIndex;
    const bySnippet = detail.events.findIndex((_, index) => anchored(index));
    if (bySnippet >= 0) return bySnippet;
    return detail.events.findIndex((event) => eventMatches(event, searchTarget.query));
  }, [detail, searchTarget]);

  const readingState = useMemo(() => ({ expanded: new Set<string>(), payloads: new Map<string, Promise<ContentBlock>>() }), [meta.sessionId, meta.projectDir]);
  const state = useMemo(() => ({ ...readingState, markdown: renderMarkdown }), [readingState, renderMarkdown]);
  const hasContext = useMemo(() => detail.events.some((ev) => ev.type === "context"), [detail]);
  const visible = useMemo(() => detail.events.flatMap((ev, index) => eventVisible(ev, showTools, showThinking, showContext, resultsByUseId) ? [index] : []), [detail, showTools, showThinking, showContext, resultsByUseId]);
  const itemKey = useCallback((index: number) => visible[index], [visible]);
  const virtual = useVirtualizer({ count: visible.length, getScrollElement: () => bodyRef.current,
    estimateSize: () => 120, overscan: 5, getItemKey: itemKey, gap: 10 });
  const markdownAnchor = useRef<number | null>(null);
  const changeMarkdown = (enabled: boolean) => {
    const offset = virtual.scrollOffset ?? 0;
    const first = virtual.getVirtualItems().find((row) => row.end > offset);
    markdownAnchor.current = first ? visible[first.index] : null;
    saveMarkdownPreference(enabled);
    setRenderMarkdown(enabled);
  };
  useLayoutEffect(() => {
    const index = markdownAnchor.current;
    if (index === null) return;
    markdownAnchor.current = null;
    virtual.measure();
    // 先让虚拟列表重建空缓存对应的布局，再恢复已挂载行，避免把旧布局当作已测量值。
    virtual.getVirtualItems();
    // 清空缓存后，尺寸未变化的已挂载行不会触发 ResizeObserver；必须立即恢复它们的真实高度。
    // 只测量可见行与少量预渲染行，避免折叠卡片长期停在 120px 估值上。
    bodyRef.current?.querySelectorAll<HTMLElement>(".event-group[data-index]").forEach((row) => {
      virtual.resizeItem(Number(row.dataset.index), row.getBoundingClientRect().height);
    });
    if (!searchTarget) {
      const position = visible.indexOf(index);
      if (position >= 0) virtual.scrollToIndex(position, { align: "start" });
    }
  }, [renderMarkdown, virtual, visible, searchTarget]);
  const navigation = useRef(0);
  const [jumpTarget, setJumpTarget] = useState(-1);
  const jumpTo = useCallback((idx: number) => {
    const position = visible.indexOf(idx);
    if (position < 0) return;
    const generation = ++navigation.current;
    setJumpTarget(idx);
    virtual.scrollToIndex(position, { align: "start" });
    // 先让虚拟行挂载并测量，再按高亮的实际位置校正；目录和全文搜索共用此路径。
    let frames = 0;
    const settle = () => {
      if (navigation.current !== generation) return;
      const row = bodyRef.current?.querySelector<HTMLElement>(`#ev-${idx}`);
      if (row && frames >= 2) {
        if (row.querySelector("[data-markdown-pending]") && frames++ < 120) { requestAnimationFrame(settle); return; }
        const mark = row.querySelector("mark.search-highlight");
        (mark ?? row).scrollIntoView({ behavior: "auto", block: mark ? "center" : "start" });
        row.classList.add("flash");
        return;
      }
      if (frames++ < 20) { virtual.scrollToIndex(position, { align: "start" }); requestAnimationFrame(settle); }
    };
    requestAnimationFrame(settle);
  }, [visible, virtual]);
  useEffect(() => {
    bodyRef.current?.scrollTo(0, 0);
    setJumpTarget(-1);
    return () => { navigation.current++; };
  }, [meta.sessionId, meta.projectDir]);
  useEffect(() => {
    if (!searchTarget) return;
    if (hitIndex < 0) { toast("会话内容已变化，未找到匹配位置，请重新搜索。", true); return; }
    jumpTo(hitIndex);
  }, [hitIndex, searchTarget, jumpTo, toast, renderMarkdown]);

  const copyResume = async () => {
    try {
      const cmd = await api.resumeCommand(agent.id, meta.sessionId, false);
      if (!cmd) {
        toast("该 agent 不支持恢复会话", true);
        return;
      }
      await writeText(cmd);
      toast("已复制恢复命令");
    } catch (e) {
      toast(String(e), true);
    }
  };

  const resume = async (fork: boolean) => {
    try {
      await api.resumeInTerminal(agent.id, meta.sessionId, meta.cwd ?? undefined, fork);
      toast(fork ? "已在新终端窗口分叉会话" : "已在新终端窗口恢复会话");
    } catch (e) {
      toast(String(e), true);
    }
  };

  const remove = async () => {
    if (removing.current) return;
    removing.current = true;
    onDeletingChange(true);
    try {
      await api.deleteSession(agent.id, meta.projectDir, meta.sessionId);
      toast(toTrash ? "会话已移入回收站" : "会话已永久删除");
      onDeleted();
    } catch (e) {
      toast(String(e), true);
    } finally {
      setConfirmOpen(false);
      removing.current = false;
      onDeletingChange(false);
    }
  };

  return (
    <DetailState.Provider value={state}><div className="detail">
      <div className="detail-header">
        <div className="detail-title">{meta.title || "(无标题)"}</div>
        <div className="detail-meta">
          <CopyButton key={sessionKey(meta)} className="session-id" value={meta.sessionId} label={`复制会话 ID：${meta.sessionId}`} title="点击复制完整会话 ID" iconClassName="copy-id-icon" onError={error => toast(`复制会话 ID 失败：${String(error)}`, true)}>
            <span>ID:</span>
            <code>{meta.sessionId}</code>
          </CopyButton>
          {meta.lastTs && <time className="session-time" dateTime={meta.lastTs} title={`${fullTime(meta.firstTs)} ~ ${fullTime(meta.lastTs)}`}>
            {fullTime(meta.lastTs)}
          </time>}
          <SessionInfo key={sessionKey(meta)} meta={meta} models={models} />
        </div>
        <div className="detail-actions">
          {meta.hasTranscript && (
            <>
              <button onClick={copyResume}>复制恢复命令</button>
              <button onClick={() => resume(false)}>在终端恢复</button>
              {agent.supportsFork && (
                <button onClick={() => resume(true)}>分叉会话</button>
              )}
            </>
          )}
          <button className="danger" onClick={() => setConfirmOpen(true)}>
            删除
          </button>
          {meta.hasTranscript && (
            <span className="view-toggles">
              <ViewToggle label="工具调用" checked={showTools} onChange={setShowTools} />
              <ViewToggle label="思考过程" checked={showThinking} onChange={setShowThinking} />
              {hasContext && <ViewToggle label="上下文" checked={showContext} onChange={setShowContext} title="查看系统指令、环境信息和压缩摘要" />}
              <ViewToggle label="Markdown" checked={renderMarkdown} onChange={changeMarkdown} />
            </span>
          )}
        </div>
      </div>
      <div className="detail-content">
        <div className="detail-body" ref={bodyRef}>
          <div className="virtual-events" style={{ height: virtual.getTotalSize() }}>
          {virtual.getVirtualItems().map((row) => {
            const i = visible[row.index];
            return <div data-index={row.index} title={agent.id === "opencode" ? eventModelTitle(detail.events[i]) : undefined} ref={virtual.measureElement} style={{ transform: `translateY(${row.start}px)` }}
              className={`event-group${i === hitIndex ? " search-target" : ""}${i === jumpTarget ? " jump-target" : ""}`} id={`ev-${i}`} key={row.key}>
              <RowKey.Provider value={String(i)}><EventView
                ev={detail.events[i]}
                resultsByUseId={resultsByUseId}
                showTools={showTools}
                showThinking={showThinking}
                renderMarkdown={renderMarkdown}
                highlightQuery={i === hitIndex ? searchTarget?.query : undefined}
              /></RowKey.Provider>
            </div>;
          })}
          </div>
          {visible.length === 0 && (
            <div className="empty-hint">
              {hasContext && !showContext ? "暂无对话正文，可勾选“上下文”查看运行记录" : detail.events.length > 0
                ? "当前显示选项下没有消息" : meta.hasTranscript
                ? "该会话暂无消息记录"
                : "这是一个空壳目录：qoder 启动或 /resume 时创建的占位会话，没有对话记录，可安全删除"}
            </div>
          )}
        </div>
        {toc.length > 1 && (
          <nav className="toc">
            <div className="toc-title">对话导览 · {toc.length} 条指令</div>
            {toc.map((t, n) => (
              <button key={t.idx} className="toc-item" onClick={() => jumpTo(t.idx)} title={t.text}>
                <span className="toc-num">{n + 1}</span>
                <span className="toc-text">{t.text}</span>
              </button>
            ))}
          </nav>
        )}
      </div>
      {confirmOpen && (
        <ConfirmDialog
          title="删除会话"
          message={
            <>
              确定删除会话「{meta.title || meta.sessionId.slice(0, 8) + "…"}」吗？
              <br />
              {toTrash
                ? "会话文件将移入系统回收站，可以随时还原。"
                : "该 agent 的删除由官方 CLI 执行，不可恢复。"}
            </>
          }
          confirmLabel={toTrash ? "移入回收站" : "永久删除"}
          onConfirm={remove}
          onCancel={() => setConfirmOpen(false)}
        />
      )}
    </div></DetailState.Provider>
  );
}
