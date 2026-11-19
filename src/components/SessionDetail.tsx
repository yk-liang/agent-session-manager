import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import CopyButton from "./CopyButton";
import ViewToggle from "./ViewToggle";
import { useVirtualizer } from "@tanstack/react-virtual";
import SessionActions from "./SessionActions";
import SessionInfo from "./SessionInfo";
import { DetailState, EventView, RowKey } from "./SessionEvent";
import { rowVisible, sessionRows } from "../sessionContent";
import { readMarkdownPreference, saveMarkdownPreference } from "../markdownPreference";
import { eventMatches, eventTextParts } from "../conversationText";
import type { AgentInfo, ContentBlock, ConversationSearchTarget, SessionDetail as Detail, SessionMeta } from "../types";
import { fullTime, sessionKey } from "../utils";
import { historicalModels, eventModelTitle } from "../sessionModels";
import ReadDiagnostics from "./ReadDiagnostics";

interface Props {
  detail: Detail;
  agent: AgentInfo;
  onRequestDelete: (session: SessionMeta) => void;
  actionsDisabled: boolean;
  toast: (msg: string, isError?: boolean) => void;
  searchTarget?: ConversationSearchTarget | null;
  onRetryRead?: () => void;
}

export default function SessionDetailView({ detail, agent, onRequestDelete, actionsDisabled, toast, searchTarget = null, onRetryRead }: Props) {
  const { meta } = detail;
  const [showTools, setShowTools] = useState(true);
  const [showThinking, setShowThinking] = useState(true);
  const [showContext, setShowContext] = useState(false);
  const [renderMarkdown, setRenderMarkdown] = useState(readMarkdownPreference);

  const rows = useMemo(() => sessionRows(detail.events), [detail.events]);
  const visibility = useMemo(() => ({ tool: showTools, thinking: showThinking, context: showContext }), [showTools, showThinking, showContext]);

  const toc = useMemo(() => {
    const items: { idx: number; text: string }[] = [];
    detail.events.forEach((ev, i) => {
      if (ev.type !== "user" || ev.isMeta) return;
      const text = eventTextParts(ev).join("\n");
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
  const visible = useMemo(() => rows.flatMap((row, index) => rowVisible(row, visibility) ? [index] : []), [rows, visibility]);
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
    virtual.getVirtualItems();
    // 尺寸未变的已挂载行不会触发 ResizeObserver，清空缓存后要重新测量。
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
    // 等虚拟行和 Markdown 挂载后，再滚到实际匹配位置。
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
    if (hitIndex < 0) { toast("找不到原来的匹配内容，请重新搜索。", true); return; }
    jumpTo(hitIndex);
  }, [hitIndex, searchTarget, jumpTo, toast, renderMarkdown]);

  return (
    <DetailState.Provider value={state}><div className="detail">
      <div className="detail-header">
        <div className="detail-title">{meta.title || "无标题"}</div>
        <div className="detail-meta">
          <CopyButton key={`copy:${sessionKey(meta)}`} className="session-id" value={meta.sessionId} label={`复制会话 ID：${meta.sessionId}`} title="复制会话 ID" iconClassName="copy-id-icon" onError={error => toast(`复制会话 ID 失败：${String(error)}`, true)}>
            <span>ID:</span>
            <code>{meta.sessionId}</code>
          </CopyButton>
          {meta.lastTs && <time className="session-time" dateTime={meta.lastTs} title={`${fullTime(meta.firstTs)} ~ ${fullTime(meta.lastTs)}`}>
            {fullTime(meta.lastTs)}
          </time>}
          <SessionInfo key={`info:${sessionKey(meta)}`} meta={meta} models={models} />
        </div>
        <div className="detail-actions">
          <SessionActions agent={agent} session={meta} onRequestDelete={onRequestDelete} toast={toast} disabled={actionsDisabled} />
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
      {detail.warnings?.length ? <ReadDiagnostics className="detail-warnings" title="部分会话内容无法读取" errors={detail.warnings} retry={onRetryRead} /> : null}
      <div className="detail-content">
        <div className="detail-body" ref={bodyRef}>
          <div className="virtual-events" style={{ height: virtual.getTotalSize() }}>
          {virtual.getVirtualItems().map((row) => {
            const i = visible[row.index];
            return <div data-index={row.index} title={agent.id === "opencode" ? eventModelTitle(detail.events[i]) : undefined} ref={virtual.measureElement} style={{ transform: `translateY(${row.start}px)` }}
              className={`event-group${i === hitIndex ? " search-target" : ""}${i === jumpTarget ? " jump-target" : ""}`} id={`ev-${i}`} key={row.key}>
              <RowKey.Provider value={String(i)}><EventView
                row={rows[i]}
                visibility={visibility}
                renderMarkdown={renderMarkdown}
                highlightQuery={i === hitIndex ? searchTarget?.query : undefined}
              /></RowKey.Provider>
            </div>;
          })}
          </div>
          {visible.length === 0 && (
            <div className="empty-hint">
              {hasContext && !showContext ? "启用「上下文」可查看运行记录。" : detail.events.length > 0
                ? "当前筛选条件下没有消息" : meta.hasTranscript
                ? "此会话暂无消息"
                : "此会话没有对话记录"}
            </div>
          )}
        </div>
        {toc.length > 1 && (
          <nav className="toc">
            <div className="toc-title">对话导览 · {toc.length} 条用户消息</div>
            {toc.map((t, n) => (
              <button key={t.idx} className="toc-item" onClick={() => jumpTo(t.idx)} title={t.text}>
                <span className="toc-num">{n + 1}</span>
                <span className="toc-text">{t.text}</span>
              </button>
            ))}
          </nav>
        )}
      </div>
    </div></DetailState.Provider>
  );
}
