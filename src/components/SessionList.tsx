import { useCallback, useLayoutEffect, useRef, type CSSProperties } from "react";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import type { AgentInfo, CatalogFilter, ConversationSearchHit, SessionDeleteFailure, SessionMeta } from "../types";
import { ChevronRight } from "lucide-react";
import UiIcon from "./UiIcon";
import { displaySessionId, formatSize, formatLocalDateTime, isEmptySession, relativeTime, sessionKey, shortPath } from "../utils";
import type { SessionListRow } from "../sessionTree";
import ModelLabel from "./ModelLabel";
import HighlightedText from "./HighlightedText";
import SearchField from "./SearchField";
import ReadDiagnostics from "./ReadDiagnostics";
import SessionActions from "./SessionActions";
import CopyButton from "./CopyButton";
import PathLabel from "./PathLabel";
import { Checkbox, CheckboxField } from "./Checkbox";

const HIT_ROLES: Record<ConversationSearchHit["role"], string> = { user: "用户", assistant: "助手", agent: "智能体" };

interface Props {
  scopeKey: string;
  rows: SessionListRow[];
  expanded: ReadonlySet<string>;
  onToggleChildren: (key: string) => void;
  hidingSessionKey?: string;
  revealSessionKey: string | null;
  onRevealComplete: () => void;
  selectedSessionKey: string | null;
  search: string;
  onSearch: (s: string) => void;
  onSelect: (s: SessionMeta) => void;
  filter: CatalogFilter;
  onFilterChange: (filter: CatalogFilter) => void;
  batchMode: boolean;
  agent?: AgentInfo;
  onRequestDelete: (session: SessionMeta) => void;
  onToggleArchive: (session: SessionMeta) => void;
  toast: (message: string, error?: boolean) => void;
  checkedSessionKeys: Set<string>;
  onToggleBatch: () => void;
  onToggleAll: () => void;
  onDeleteSelected: () => void;
  disabled: boolean;
  loading: boolean;
  refreshing: boolean;
  loadError: string | null;
  warnings: string[];
  onRetryLoad: () => void;
  failures: SessionDeleteFailure[];
  searchHits: Map<string, ConversationSearchHit>;
  searchPending: boolean;
  searchSkipped: number;
  searchError: string | null;
  onRetrySearch: () => void;
  showPaths: boolean;
  hiddenPathSessionKeys?: ReadonlySet<string>;
  displayRoots?: ReadonlyMap<string, string | null>;
}

export default function SessionList({
  scopeKey,
  rows,
  expanded,
  onToggleChildren,
  hidingSessionKey,
  revealSessionKey,
  onRevealComplete,
  selectedSessionKey,
  search,
  onSearch,
  onSelect,
  filter,
  onFilterChange,
  batchMode,
  agent,
  onRequestDelete,
  onToggleArchive,
  toast,
  checkedSessionKeys,
  onToggleBatch,
  onToggleAll,
  onDeleteSelected,
  disabled,
  loading,
  refreshing,
  loadError,
  warnings,
  onRetryLoad,
  failures,
  searchHits,
  searchPending,
  searchSkipped,
  searchError,
  onRetrySearch,
  showPaths,
  hiddenPathSessionKeys,
  displayRoots,
}: Props) {
  const busy = loading || refreshing;
  const selectedCount = batchMode && checkedSessionKeys.size
    ? new Set(rows.filter(row => checkedSessionKeys.has(row.key)).map(row => row.key)).size : 0;
  const allSelected = selectedCount > 0 && rows.every(row => checkedSessionKeys.has(row.key));
  const scroll = useRef<HTMLDivElement>(null);
  const getItemKey = useCallback((index: number) => `${agent?.id}:${rows[index].key}`, [agent?.id, rows]);
  const list = useVirtualizer({ count: rows.length, getScrollElement: () => scroll.current,
    getItemKey, estimateSize: () => 76, gap: 6, paddingStart: 8, paddingEnd: 8, overscan: 5,
    useAnimationFrameWithResizeObserver: true });
  const items = list.getVirtualItems();
  useLayoutEffect(() => { scroll.current?.scrollTo(0, 0); }, [scopeKey, search]);
  // 滚动后用首条可见会话定位，列表顶部则跟随最新排序。
  const layout = useRef<{ scope: string; rows: SessionListRow[]; items: Pick<VirtualItem, "key" | "start" | "end">[] } | null>(null);
  const layoutScope = `${scopeKey}\n${search}`;
  useLayoutEffect(() => {
    const previous = layout.current;
    const element = scroll.current;
    const top = element?.scrollTop ?? 0;
    if (previous && previous.scope === layoutScope && previous.rows !== rows && top > 0) {
      for (const item of previous.items) {
        if (item.end <= top) continue;
        const index = rows.findIndex((_, i) => getItemKey(i) === item.key);
        if (index < 0) continue;
        const start = list.measurementsCache[index]?.start;
        if (start !== undefined && start !== item.start) list.scrollBy(start - item.start);
        break;
      }
    }
    layout.current = { scope: layoutScope, rows, items };
  });
  const revealIndex = revealSessionKey ? rows.findIndex(row => row.key === revealSessionKey) : -1;
  const revealMounted = items.some(item => item.index === revealIndex);
  useLayoutEffect(() => {
    if (revealIndex >= 0) list.scrollToIndex(revealIndex, { align: "auto" });
  }, [list, revealIndex, revealSessionKey]);
  useLayoutEffect(() => {
    if (!revealMounted) return;
    scroll.current?.querySelector<HTMLElement>(`[data-index="${revealIndex}"] :is(.session-card-open, .session-card-selection input)`)?.focus({ preventScroll: true });
    onRevealComplete();
  }, [revealIndex, revealMounted, onRevealComplete]);
  const emptyHint = loading ? "正在读取会话…"
    : loadError || warnings.length > 0 ? "暂无可显示的会话"
    : search.trim() ? "未找到匹配的会话" : "暂无会话";
  return (
    <div className="session-list" aria-busy={busy}>
      <div className="search-box">
        <SearchField placeholder="搜索会话或正文…" aria-label="搜索会话或正文"
          maxLength={256} value={search} onChange={onSearch} disabled={disabled} />
        {(searchPending || searchSkipped > 0 || searchError) && (
          <div className={`search-status${searchSkipped || searchError ? " warning" : ""}`} role="status" title={searchError ?? undefined}>
            {searchPending ? "正在搜索正文…" : searchError ? <>无法搜索正文 <button type="button" onClick={onRetrySearch} disabled={disabled}>重试</button></> : `无法搜索 ${searchSkipped} 个会话`}
          </div>
        )}
        {loadError && <ReadDiagnostics title="无法读取会话列表" errors={[loadError]} retry={onRetryLoad} busy={disabled || busy} />}
        {warnings.length > 0 && <ReadDiagnostics title="部分会话无法读取" errors={warnings} retry={onRetryLoad} busy={disabled || busy} />}
        <div className="session-list-options">
          <div className="session-list-filters">
            <CheckboxField label="空会话" checked={filter.includeEmpty} disabled={disabled}
              onChange={includeEmpty => onFilterChange({ ...filter, includeEmpty })} />
            {agent?.supportsArchivedSessions && (
              <CheckboxField label="已归档" checked={filter.includeArchived} disabled={disabled}
                onChange={includeArchived => onFilterChange({ ...filter, includeArchived })} />
            )}
          </div>
          {agent && agent.deletePolicy !== "readOnly" && (
            <button type="button" onClick={onToggleBatch} disabled={disabled || busy || searchPending || (!batchMode && rows.length === 0)}>
              {batchMode ? "完成" : "批量管理"}
            </button>
          )}
        </div>
      </div>
      {batchMode && (
        <div className="batch-toolbar">
          <div className="batch-status" role="status">
            已选中 {selectedCount} 个会话
          </div>
          <div className="batch-actions">
            <button type="button" onClick={onToggleAll} disabled={disabled || busy || searchPending || rows.length === 0}>
              {allSelected ? "取消全选" : "全选结果"}
            </button>
            <button type="button" className="danger" onClick={onDeleteSelected} disabled={disabled || busy || searchPending || selectedCount === 0}>
              删除所选
            </button>
          </div>
          <p className="batch-hint">更改筛选会清除已选会话。</p>
        </div>
      )}
      {failures.length > 0 && (
        <details className="batch-failures" open>
          <summary><UiIcon icon={ChevronRight} className="details-caret" />{failures.length} {agent?.id === "codex" ? "项删除失败" : "个会话未删除"}</summary>
          <ul>
            {failures.map(({ session, error }) => (
              <li key={sessionKey(session)}>
                <strong>{session.title || displaySessionId(session)}</strong>
                <div className="session-location"><span>{shortPath(session.cwd) || session.projectDir}</span><code>{displaySessionId(session)}</code></div>
                <div>{error}</div>
              </li>
            ))}
          </ul>
        </details>
      )}
      <div className="session-cards" ref={scroll}>
        <div className="session-card-list" style={{ height: rows.length ? list.getTotalSize() : undefined }}>
          {items.map(item => {
            const { session: s, key, depth, childCount, guides } = rows[item.index];
            const statsKnown = !s.statsPending || s.statsRetained;
            const isEmpty = statsKnown && isEmptySession(s);
            const hit = searchHits.get(key);
            const title = s.title || "无标题";
            const hidePath = hiddenPathSessionKeys?.has(key) ?? false;
            const displayPath = displayRoots?.get(key) ?? s.cwd;
            const copyPath = s.cwd ?? displayPath;
            const cardTitle = hidePath ? s.cwd ?? undefined : undefined;
            const orphan = !!s.subagent?.orphan;
            const cardClass = `session-card${selectedSessionKey === key ? " active" : ""}${isEmpty ? " empty" : ""}${s.archived ? " archived" : ""}${orphan ? " orphan" : ""}${depth ? " child" : ""}`;
            const body = (
              <>
                <span className="session-card-header">
                  <span className="session-card-title">{title}</span>
                  <time className="session-card-time" dateTime={s.lastTs ?? undefined} title={formatLocalDateTime(s.lastTs)}>
                    {relativeTime(s.lastTs)}
                  </time>
                </span>
                {hit && <span className="session-search-snippet"><span>{HIT_ROLES[hit.role]}</span><HighlightedText text={hit.snippet} query={search} /></span>}
                <span className="session-card-sub">
                  {s.readError && <span className="status-tag" data-tone="error" title="打开会话以重试或查看诊断">正文读取失败</span>}
                  {isEmpty && <span className="status-tag" data-tone="info">空会话</span>}
                  {s.codex?.source === "exec" && <span className="status-tag" data-tone="success">exec</span>}
                  <ModelLabel model={s.model} badge />
                  <span className="session-card-stats">
                    {statsKnown && <span>{s.realUserCount + s.assistantTextCount} 条消息</span>}
                    {s.sizeBytes != null && <span className="session-card-size">{formatSize(s.sizeBytes)}</span>}
                  </span>
                </span>
                {!!search.trim() && s.subagent?.parentId && <span className="session-search-parent">父会话 ID <code>{s.subagent.parentId}</code></span>}
                {!search.trim() && !depth && s.subagent && !orphan &&
                  <span className="session-search-parent">父会话不在当前范围</span>}
                {(s.archived || orphan) && <span className={`archived-ribbon${orphan ? " orphan-ribbon" : ""}`}><span>{orphan ? "孤儿" : "已归档"}</span></span>}
                {s.archived && orphan && <span className="session-search-parent">已归档</span>}
              </>
            );
            const pathButton = showPaths && !hidePath && displayPath && copyPath ? (
              <CopyButton
                className="status-tag session-project-path"
                value={copyPath}
                label={`复制路径：${copyPath}`}
                title={copyPath}
                iconClassName="session-path-copy-icon"
                onError={error => toast(`复制路径失败：${String(error)}`, true)}
              >
                <PathLabel path={shortPath(displayPath)} className="session-project-path-text" bounds=".session-path-slot" reservedWidth={14} />
              </CopyButton>
            ) : null;
            const canToggle = childCount > 0;
            const isExpanded = expanded.has(key);
            const hidesSelection = hidingSessionKey === key;
            const footer = canToggle || pathButton ? (
              <div className="session-card-path">
                {canToggle && <button type="button" className="status-tag session-children-toggle"
                  aria-expanded={isExpanded}
                  aria-label={`${isExpanded ? "收起" : "展开"} ${title} 的 ${childCount} 个子会话${hidesSelection ? "（包含正在查看的会话）" : ""}`}
                  onClick={() => onToggleChildren(key)} disabled={disabled}>
                  <UiIcon icon={ChevronRight} size={12} />
                  {childCount} 个子会话
                  {hidesSelection && <span className="session-children-dot" aria-hidden="true" />}
                </button>}
                {pathButton && <div className="session-path-slot">{pathButton}</div>}
              </div>
            ) : null;
            return (
              <div className="session-card-row" key={item.key} data-index={item.index}
                data-session-depth={depth || undefined}
                ref={list.measureElement} style={{ top: item.start, "--session-depth": guides.length } as CSSProperties}>
                {guides.map((continues, index) => <span key={index} aria-hidden="true"
                  className={`session-branch${index === guides.length - 1 ? " elbow" : ""}${continues ? " continues" : ""}`}
                  style={{ left: 8 + index * 18 }} />)}
                <div className={cardClass} title={cardTitle}>
                  {batchMode ? (
                    <label className="session-card-selection">
                      <Checkbox checked={checkedSessionKeys.has(key)} onChange={() => onSelect(s)}
                        disabled={disabled || busy || searchPending}
                        aria-label={`选择会话：${title}（${s.projectDir}）`} />
                      <span className="session-card-content">{body}</span>
                    </label>
                  ) : (
                    <>
                      <button type="button" className="session-card-open" onClick={() => onSelect(s)} disabled={disabled}>
                        {body}
                      </button>
                      {agent && <SessionActions agent={agent} session={s} onRequestDelete={onRequestDelete}
                        onToggleArchive={onToggleArchive} toast={toast} compact disabled={disabled} />}
                    </>
                  )}
                  {footer}
                </div>
              </div>
            );
          })}
          {rows.length === 0 && (loading || !searchPending) && <div className="empty-hint" role="status">{emptyHint}</div>}
        </div>
      </div>
    </div>
  );
}
