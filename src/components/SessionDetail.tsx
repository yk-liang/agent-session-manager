import { useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, type MouseEvent } from "react";
import { ArrowDown, ChevronDown } from "lucide-react";
import CopyButton from "./CopyButton";
import { CheckboxField } from "./Checkbox";
import { useVirtualizer } from "@tanstack/react-virtual";
import SessionActions from "./SessionActions";
import SessionInfo from "./SessionInfo";
import { useSessionOutline } from "./SessionOutline";
import { useConversationScroll } from "../useConversationScroll";
import { DetailState } from "./DetailState";
import { EventView } from "./SessionEvent";
import { DEFAULT_VISIBILITY, rowEndsWithMessage, sessionContent, visibleSegments, type SessionReading, type VisibleRow } from "../sessionContent";
import { useUiPreference } from "../useUiPreference";
import { eventMatches, eventTextParts } from "../conversationText";
import type { AgentInfo, ConversationSearchTarget, SessionDetail as Detail, SessionMeta } from "../types";
import { displaySessionId, formatLocalDateTime, formatMessageTimestamp, sessionKey } from "../utils";
import { historicalModels } from "../models";
import ReadDiagnostics from "./ReadDiagnostics";
import UiIcon from "./UiIcon";

interface Props {
  detail: Detail;
  reading: SessionReading;
  agent: AgentInfo;
  onRequestDelete: (session: SessionMeta) => void;
  onToggleArchive: (session: SessionMeta) => void;
  onOpenParent?: () => void;
  onReveal?: () => void;
  actionsDisabled: boolean;
  toast: (msg: string, isError?: boolean) => void;
  searchTarget?: ConversationSearchTarget | null;
  onRetryRead?: () => void;
}

/** 虚拟列表的一行：会话事件，或没有事件的分叉线。键是事件序号，分叉线取继承事件与子会话事件之间的位置。 */
type ListItem = { key: number; row?: VisibleRow };

const INHERITED_OPEN = "inherited";

function SessionId({ id, parent = false, onError }: { id: string; parent?: boolean; onError: (error: unknown) => void }) {
  const label = parent ? "父会话 ID" : "会话 ID";
  return (
    <CopyButton className="session-id btn-ghost btn-sm" value={id} label={`复制${label}：${id}`}
      title={`复制${label}`} iconClassName="copy-id-icon" onError={onError}>
      <span>{parent ? "父会话 ID:" : "ID:"}</span>
      <code>{id}</code>
    </CopyButton>
  );
}

export default function SessionDetailView({ detail, reading, agent, onRequestDelete, onToggleArchive, onOpenParent, onReveal, actionsDisabled, toast, searchTarget = null, onRetryRead }: Props) {
  const { meta } = detail;
  const key = sessionKey(meta);
  const parentId = meta.subagent?.parentId;
  const reportCopyError = useCallback((error: unknown) => toast(`复制失败：${String(error)}`, true), [toast]);
  const [{ tool: showTools, thinking: showThinking, context: showContext, markdown: renderMarkdown }, setConversation] = useUiPreference("conversation");

  const content = sessionContent(detail.events, detail.inherited);
  const { rows, toc, hasContext, inherited } = content;
  const visibility = useMemo(() => showTools && showThinking && !showContext ? DEFAULT_VISIBILITY
    : { tool: showTools, thinking: showThinking, context: showContext }, [showTools, showThinking, showContext]);

  const models = useMemo(() => historicalModels(detail.events), [detail.events]);
  const bodyRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  // 正文搜索不含继承的上下文：命中序号从子会话自己的事件算起，也只在这些事件中查找。
  const hitIndex = useMemo(() => {
    if (!searchTarget) return -1;
    const matches = (index: number) => !!detail.events[index] && eventMatches(detail.events[index], searchTarget.query);
    const find = (predicate: (index: number) => boolean) => {
      for (let index = inherited; index < detail.events.length; index++) if (predicate(index)) return index;
      return -1;
    };
    const byId = searchTarget.eventId ? find(index => detail.events[index].uuid === searchTarget.eventId) : -1;
    if (matches(byId)) return byId;
    const anchor = searchTarget.snippet.replace(/^…|…$/g, "");
    const anchored = (index: number) => matches(index) && eventTextParts(detail.events[index]).some((text) => text.includes(anchor));
    if (anchored(searchTarget.eventIndex + inherited)) return searchTarget.eventIndex + inherited;
    const bySnippet = find(anchored);
    return bySnippet >= 0 ? bySnippet : find(matches);
  }, [detail.events, inherited, searchTarget]);

  const state = useMemo(() => ({ ...reading, markdown: renderMarkdown, cwd: meta.cwd, onCopyError: reportCopyError }),
    [reading, renderMarkdown, meta.cwd, reportCopyError]);
  const segments = useMemo(() => visibility === DEFAULT_VISIBILITY ? content.visible
    : visibleSegments(content.rows, visibility, inherited), [content, visibility, inherited]);
  const [, rerender] = useReducer((count: number) => count + 1, 0);
  const inheritedOpen = reading.expanded.get(INHERITED_OPEN) ?? false;
  const fork = useMemo(() => {
    if (!inherited) return;
    const time = detail.events.find((event, index) => index >= inherited && event.timestamp)?.timestamp;
    return { at: inherited - 0.5, title: time && `${formatMessageTimestamp(time)} 从父会话分叉` };
  }, [detail.events, inherited]);
  const items = useMemo((): ListItem[] => {
    const own = segments.own.map(row => ({ key: row.index, row }));
    if (!fork) return own;
    const parent = inheritedOpen ? segments.inherited.map(row => ({ key: row.index, row })) : [];
    return [...parent, { key: fork.at }, ...own];
  }, [segments, fork, inheritedOpen]);
  const entries = useMemo(() => inheritedOpen ? toc : toc.filter(entry => entry.idx >= inherited), [toc, inheritedOpen, inherited]);
  const heads = useMemo(() => items.map(item => item.key), [items]);
  const groups = useMemo(() => items.map(({ row }) => row ? [row.index, ...row.members].map(i => rows[i]) : []), [items, rows]);
  const itemKey = useCallback((index: number) => items[index].key, [items]);
  const virtual = useVirtualizer({ count: items.length, getScrollElement: () => bodyRef.current,
    estimateSize: () => 120, overscan: 5, getItemKey: itemKey, gap: 8, useAnimationFrameWithResizeObserver: true });
  const { position, jumpTo, jumpToBottom } = useConversationScroll(key, entries, heads, virtual, bodyRef, contentRef);
  const outline = useSessionOutline(entries, key, position, jumpTo, fork);
  // 切换后定位到分叉线：展开时它下移到视口约三分之一处，露出分叉前的最后几条消息；收起后回到正文顶部。
  const forkJump = useRef<{ offset?: number } | null>(null);
  const toggleInherited = ({ currentTarget }: MouseEvent<HTMLElement>) => {
    const viewport = bodyRef.current, row = currentTarget.parentElement;
    forkJump.current = inheritedOpen || !viewport || !row ? {} : {
      offset: Math.max(row.getBoundingClientRect().top - viewport.getBoundingClientRect().top, viewport.clientHeight / 3),
    };
    reading.expanded.set(INHERITED_OPEN, !inheritedOpen);
    rerender();
  };
  useLayoutEffect(() => {
    const options = forkJump.current;
    if (!options || !fork) return;
    forkJump.current = null;
    jumpTo(fork.at, options);
  }, [fork, jumpTo]);
  const markdownAnchor = useRef<number | null>(null);
  const changeMarkdown = (enabled: boolean) => {
    const offset = virtual.scrollOffset ?? 0;
    const first = virtual.getVirtualItems().find((row) => row.end > offset);
    markdownAnchor.current = first ? items[first.index].key : null;
    setConversation(current => ({ ...current, markdown: enabled }));
  };
  useLayoutEffect(() => {
    const anchor = markdownAnchor.current;
    if (anchor === null) return;
    markdownAnchor.current = null;
    virtual.measure();
    virtual.getVirtualItems();
    // 尺寸未变的已挂载行不会触发 ResizeObserver，清空缓存后要重新测量。
    bodyRef.current?.querySelectorAll<HTMLElement>(".event-group[data-index]").forEach((row) => {
      virtual.resizeItem(Number(row.dataset.index), row.getBoundingClientRect().height);
    });
    if (!searchTarget) {
      const position = items.findIndex(item => item.key === anchor);
      if (position >= 0) virtual.scrollToIndex(position, { align: "start" });
    }
  }, [renderMarkdown, virtual, items, searchTarget]);
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
          {meta.archived && <span className="status-tag">已归档</span>}
          {meta.subagent?.orphan && <span className="status-tag" data-tone="warning">孤儿子会话</span>}
          <SessionId key={key} id={displaySessionId(meta)} onError={reportCopyError} />
          {meta.subagent && (parentId ? (
            <SessionId key={`parent:${key}`} id={parentId} parent onError={reportCopyError} />
          ) : <span>父会话 ID 未知</span>)}
          {meta.lastTs && <time className="session-time" dateTime={meta.lastTs} title={`${formatLocalDateTime(meta.firstTs)} ~ ${formatLocalDateTime(meta.lastTs)}`}>
            {formatLocalDateTime(meta.lastTs)}
          </time>}
          <div className="detail-meta-actions">
            {outline.trigger}
            <SessionInfo key={`info:${key}`} meta={meta} agentName={agent.displayName} models={models} onCopyError={reportCopyError} />
          </div>
        </div>
        <div className="detail-toolbar">
          <div className="detail-actions">
            <SessionActions agent={agent} session={meta} onRequestDelete={onRequestDelete}
              onToggleArchive={onToggleArchive} toast={toast} disabled={actionsDisabled} />
            {onOpenParent && <button type="button" onClick={onOpenParent} disabled={actionsDisabled}
              aria-label={`打开父会话 ${parentId}`}>打开父会话</button>}
            {onReveal && <button type="button" onClick={onReveal} disabled={actionsDisabled}>在列表定位</button>}
          </div>
          {meta.hasTranscript && (
            <div className="view-toggles" role="group" aria-label="对话显示选项">
              <CheckboxField label="工具调用" checked={showTools} onChange={tool => setConversation(current => ({ ...current, tool }))} />
              <CheckboxField label="思考过程" checked={showThinking} onChange={thinking => setConversation(current => ({ ...current, thinking }))} />
              {hasContext && <CheckboxField label="上下文" checked={showContext} onChange={context => setConversation(current => ({ ...current, context }))} title="显示系统指令、环境信息和压缩摘要" />}
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
                const item = items[row.index];
                const placement = { "data-index": row.index, ref: virtual.measureElement, style: { transform: `translateY(${row.start}px)` } };
                if (!item.row) {
                  return <div {...placement} tabIndex={-1} title={fork?.title} className="event-group fork-line" id={`ev-${item.key}`} key={row.key}>
                    <button type="button" className="btn-sm" aria-expanded={inheritedOpen} onClick={toggleInherited}>
                      {inheritedOpen ? "收起分叉前的消息" : `显示分叉前的 ${content.inheritedMessages} 条消息`}
                      <UiIcon icon={ChevronDown} size={14} className="fork-chevron" />
                    </button>
                  </div>;
                }
                const i = item.row.index;
                const previous = items[row.index - 1]?.row;
                return <div {...placement} tabIndex={-1}
                  data-follows-activity={previous && !rowEndsWithMessage(rows, previous, visibility) || undefined}
                  className={`event-group${i === hitIndex ? " search-target" : ""}`} id={`ev-${i}`} key={row.key}>
                  <EventView
                    index={i}
                    rows={groups[row.index]}
                    visibility={visibility}
                    renderMarkdown={renderMarkdown}
                    highlightQuery={i === hitIndex ? searchTarget?.query : undefined}
                  />
                </div>;
              })}
            </div>
            {items.length === 0 && (
              <div className="empty-hint">
                {hasContext && !showContext ? "勾选「上下文」查看运行记录。" : detail.events.length > 0
                  ? "当前筛选下没有消息" : meta.hasTranscript
                  ? "暂无对话消息"
                  : "此会话没有正文记录"}
              </div>
            )}
          </div>
          {items.length > 0 && !position.atBottom && (
            <button type="button" className="detail-jump-bottom btn-icon" title="回到底部" aria-label="回到底部" onClick={jumpToBottom}>
              <UiIcon icon={ArrowDown} size={16} />
            </button>
          )}
        </div>
        {outline.navigation}
      </div>
    </div></DetailState.Provider>
  );
}
