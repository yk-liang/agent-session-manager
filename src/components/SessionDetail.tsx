import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ArrowDown } from "lucide-react";
import CopyButton from "./CopyButton";
import { CheckboxField } from "./Checkbox";
import { useVirtualizer } from "@tanstack/react-virtual";
import SessionActions from "./SessionActions";
import SessionInfo from "./SessionInfo";
import { useSessionOutline } from "./SessionOutline";
import { useConversationScroll } from "../useConversationScroll";
import { DetailState, EventView, RowKey } from "./SessionEvent";
import { DEFAULT_VISIBILITY, rowEndsWithMessage, sessionContent, visibleSessionRows, type SessionReading } from "../sessionContent";
import { readMarkdownPreference, saveMarkdownPreference } from "../markdownPreference";
import { eventMatches, eventTextParts } from "../conversationText";
import type { AgentInfo, ConversationSearchTarget, SessionDetail as Detail, SessionMeta } from "../types";
import { formatLocalDateTime, sessionKey } from "../utils";
import { historicalModels, eventModelTitle } from "../models";
import ReadDiagnostics from "./ReadDiagnostics";
import UiIcon from "./UiIcon";

interface Props {
  detail: Detail;
  reading: SessionReading;
  agent: AgentInfo;
  onRequestDelete: (session: SessionMeta) => void;
  onToggleArchive: (session: SessionMeta) => void;
  actionsDisabled: boolean;
  toast: (msg: string, isError?: boolean) => void;
  searchTarget?: ConversationSearchTarget | null;
  onRetryRead?: () => void;
}

export default function SessionDetailView({ detail, reading, agent, onRequestDelete, onToggleArchive, actionsDisabled, toast, searchTarget = null, onRetryRead }: Props) {
  const { meta } = detail;
  const key = sessionKey(meta);
  const reportCopyError = useCallback((error: unknown) => toast(`复制失败：${String(error)}`, true), [toast]);
  const [showTools, setShowTools] = useState(true);
  const [showThinking, setShowThinking] = useState(true);
  const [showContext, setShowContext] = useState(false);
  const [renderMarkdown, setRenderMarkdown] = useState(readMarkdownPreference);

  const content = sessionContent(detail.events);
  const { rows, toc, hasContext } = content;
  const visibility = useMemo(() => showTools && showThinking && !showContext ? DEFAULT_VISIBILITY
    : { tool: showTools, thinking: showThinking, context: showContext }, [showTools, showThinking, showContext]);

  const models = useMemo(() => agent.id === "opencode" ? historicalModels(detail.events) : [], [agent.id, detail.events]);
  const bodyRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
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
  }, [detail.events, searchTarget]);

  const state = useMemo(() => ({ ...reading, markdown: renderMarkdown, onCopyError: reportCopyError }),
    [reading, renderMarkdown, reportCopyError]);
  const visible = useMemo(() => visibility === DEFAULT_VISIBILITY ? content.visible
    : visibleSessionRows(content.rows, visibility), [content, visibility]);
  const itemKey = useCallback((index: number) => visible[index], [visible]);
  const virtual = useVirtualizer({ count: visible.length, getScrollElement: () => bodyRef.current,
    estimateSize: () => 120, overscan: 5, getItemKey: itemKey, gap: 8, useAnimationFrameWithResizeObserver: true });
  const { position, jumpTo, jumpToBottom } = useConversationScroll(key, toc, visible, virtual, bodyRef, contentRef);
  const outline = useSessionOutline(toc, key, position, jumpTo);
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
  // 正文刷新只更新定位数据，避免把读者拉回搜索命中处。
  const latestHit = useRef({ hitIndex, jumpTo });
  useLayoutEffect(() => { latestHit.current = { hitIndex, jumpTo }; }, [hitIndex, jumpTo]);
  useEffect(() => {
    if (!searchTarget) return;
    const { hitIndex, jumpTo } = latestHit.current;
    if (hitIndex < 0) { toast("会话内容已变化，请重新搜索。", true); return; }
    jumpTo(hitIndex, { match: true });
  }, [searchTarget, toast, renderMarkdown]);

  return (
    <DetailState.Provider value={state}><div className="detail" onKeyDown={outline.onKeyDown}>
      <div className="detail-header">
        <div className="detail-title">{meta.title || "无标题"}</div>
        <div className="detail-meta">
          {meta.archived && <span className="status-tag muted">已归档</span>}
          <CopyButton key={`copy:${key}`} className="session-id" value={meta.sessionId} label={`复制会话 ID：${meta.sessionId}`} title="复制会话 ID" iconClassName="copy-id-icon" onError={error => toast(`复制会话 ID 失败：${String(error)}`, true)}>
            <span>ID:</span>
            <code>{meta.sessionId}</code>
          </CopyButton>
          {meta.lastTs && <time className="session-time" dateTime={meta.lastTs} title={`${formatLocalDateTime(meta.firstTs)} ~ ${formatLocalDateTime(meta.lastTs)}`}>
            {formatLocalDateTime(meta.lastTs)}
          </time>}
          <div className="detail-meta-actions">
            {outline.trigger}
            <SessionInfo key={`info:${key}`} meta={meta} models={models} />
          </div>
        </div>
        <div className="detail-toolbar">
          <div className="detail-actions">
            <SessionActions agent={agent} session={meta} onRequestDelete={onRequestDelete}
              onToggleArchive={onToggleArchive} toast={toast} disabled={actionsDisabled} />
          </div>
          {meta.hasTranscript && (
            <div className="view-toggles" role="group" aria-label="对话显示选项">
              <CheckboxField label="工具调用" checked={showTools} onChange={setShowTools} />
              <CheckboxField label="思考过程" checked={showThinking} onChange={setShowThinking} />
              {hasContext && <CheckboxField label="上下文" checked={showContext} onChange={setShowContext} title="显示系统指令、环境信息和压缩摘要" />}
              <CheckboxField label="Markdown" checked={renderMarkdown} onChange={changeMarkdown} />
            </div>
          )}
        </div>
      </div>
      {detail.warnings?.length ? <ReadDiagnostics className="detail-warnings" title="部分会话内容无法读取" errors={detail.warnings} retry={onRetryRead} /> : null}
      <div className="detail-content">
        <div className="detail-conversation">
          <div className="detail-body" ref={bodyRef} tabIndex={-1}>
            <div className="virtual-events" ref={contentRef} style={{ height: virtual.getTotalSize() }}>
              {virtual.getVirtualItems().map((row) => {
                const i = visible[row.index];
                const previous = rows[visible[row.index - 1]];
                return <div data-index={row.index} tabIndex={-1} title={agent.id === "opencode" ? eventModelTitle(detail.events[i]) : undefined} ref={virtual.measureElement} style={{ transform: `translateY(${row.start}px)` }}
                  data-follows-activity={previous && !rowEndsWithMessage(previous, visibility) || undefined}
                  className={`event-group${i === hitIndex ? " search-target" : ""}`} id={`ev-${i}`} key={row.key}>
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
                {hasContext && !showContext ? "勾选「上下文」查看运行记录。" : detail.events.length > 0
                  ? "当前筛选下没有消息" : meta.hasTranscript
                  ? "暂无对话消息"
                  : "此会话没有正文记录"}
              </div>
            )}
          </div>
          {visible.length > 0 && !position.atBottom && (
            <button type="button" className="detail-jump-bottom" title="回到底部" aria-label="回到底部" onClick={jumpToBottom}>
              <UiIcon icon={ArrowDown} size={16} />
            </button>
          )}
        </div>
        {outline.navigation}
      </div>
    </div></DetailState.Provider>
  );
}
