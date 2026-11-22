import { useCallback, useLayoutEffect, useRef } from "react";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import type { AgentInfo, CatalogFilter, ConversationSearchHit, SessionDeleteFailure, SessionMeta } from "../types";
import { ChevronRight } from "lucide-react";
import UiIcon from "./UiIcon";
import { formatSize, formatLocalDateTime, relativeTime, sessionKey, shortPath } from "../utils";
import ModelLabel from "./ModelLabel";
import HighlightedText from "./HighlightedText";
import SearchField from "./SearchField";
import ReadDiagnostics from "./ReadDiagnostics";
import SessionActions from "./SessionActions";
import CopyButton from "./CopyButton";
import { Checkbox, CheckboxField } from "./Checkbox";

interface Props {
  scopeKey: string;
  sessions: SessionMeta[];
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
  sessions,
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
    ? new Set(sessions.filter(session => checkedSessionKeys.has(sessionKey(session))).map(sessionKey)).size : 0;
  const allSelected = selectedCount > 0 && sessions.every(session => checkedSessionKeys.has(sessionKey(session)));
  const scroll = useRef<HTMLDivElement>(null);
  const getItemKey = useCallback((index: number) => `${agent?.id}:${sessionKey(sessions[index])}`, [agent?.id, sessions]);
  const list = useVirtualizer({ count: sessions.length, getScrollElement: () => scroll.current,
    getItemKey, estimateSize: () => 76, gap: 6, paddingStart: 8, paddingEnd: 8, overscan: 5,
    useAnimationFrameWithResizeObserver: true });
  useLayoutEffect(() => { scroll.current?.scrollTo(0, 0); }, [scopeKey, search]);
  // 滚动后用首条可见会话定位，列表顶部则跟随最新排序。
  const layout = useRef<{ scope: string; sessions: SessionMeta[]; items: Pick<VirtualItem, "key" | "start" | "end">[] } | null>(null);
  const layoutScope = `${scopeKey}\n${search}`;
  useLayoutEffect(() => {
    const previous = layout.current;
    const element = scroll.current;
    const top = element?.scrollTop ?? 0;
    if (previous && previous.scope === layoutScope && previous.sessions !== sessions && top > 0) {
      for (const item of previous.items) {
        if (item.end <= top) continue;
        const index = sessions.findIndex((_, i) => getItemKey(i) === item.key);
        if (index < 0) continue;
        const start = list.measurementsCache[index]?.start;
        if (start !== undefined && start !== item.start) list.scrollBy(start - item.start);
        break;
      }
    }
    layout.current = { scope: layoutScope, sessions, items: list.getVirtualItems().map(({ key, start, end }) => ({ key, start, end })) };
  });
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
            <CheckboxField label="显示空会话" checked={filter.includeEmpty} disabled={disabled}
              onChange={includeEmpty => onFilterChange({ ...filter, includeEmpty })} />
            {agent?.supportsArchivedSessions && (
              <CheckboxField label="显示已归档" checked={filter.includeArchived} disabled={disabled}
                onChange={includeArchived => onFilterChange({ ...filter, includeArchived })} />
            )}
          </div>
          {agent && agent.deletePolicy !== "readOnly" && (
            <button type="button" onClick={onToggleBatch} disabled={disabled || busy || searchPending || (!batchMode && sessions.length === 0)}>
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
            <button type="button" onClick={onToggleAll} disabled={disabled || busy || searchPending || sessions.length === 0}>
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
          <summary><UiIcon icon={ChevronRight} className="details-caret" />{failures.length} 个会话未删除</summary>
          <ul>
            {failures.map(({ session, error }) => (
              <li key={sessionKey(session)}>
                <strong>{session.title || session.sessionId}</strong>
                <div className="session-location"><span>{shortPath(session.cwd) || session.projectDir}</span><code>{session.sessionId}</code></div>
                <div>{error}</div>
              </li>
            ))}
          </ul>
        </details>
      )}
      <div className="session-cards" ref={scroll}>
        <div className="session-card-list" style={{ height: sessions.length ? list.getTotalSize() : undefined }}>
          {list.getVirtualItems().map(item => {
            const s = sessions[item.index];
            const statsKnown = !s.statsPending || s.statsRetained;
            const isEmpty = statsKnown && s.realUserCount === 0;
            const key = sessionKey(s);
            const hit = searchHits.get(key);
            const title = s.title || "无标题";
            const hidePath = hiddenPathSessionKeys?.has(key) ?? false;
            const displayPath = displayRoots?.get(key) ?? s.cwd;
            const copyPath = s.cwd ?? displayPath;
            const cardTitle = hidePath ? s.cwd ?? undefined : undefined;
            const cardClass = `session-card${selectedSessionKey === key ? " active" : ""}${isEmpty ? " empty" : ""}${s.archived ? " archived" : ""}`;
            const body = (
              <>
                <span className="session-card-header">
                  <span className="session-card-title">{title}</span>
                  <time className="session-card-time" dateTime={s.lastTs ?? undefined} title={formatLocalDateTime(s.lastTs)}>
                    {relativeTime(s.lastTs)}
                  </time>
                </span>
                {hit && <span className="session-search-snippet"><span>{hit.role === "user" ? "用户" : "助手"}</span><HighlightedText text={hit.snippet} query={search} /></span>}
                <span className="session-card-sub">
                  {s.readError && <span className="status-tag" data-tone="error" title="打开会话以重试或查看诊断">正文读取失败</span>}
                  {isEmpty && <span className="status-tag" data-tone="info">空会话</span>}
                  <ModelLabel model={s.model} badge />
                  <span className="session-card-stats">
                    {statsKnown && <span>{s.realUserCount + s.assistantTextCount} 条消息</span>}
                    {s.sizeBytes != null && <span className="session-card-size">{formatSize(s.sizeBytes)}</span>}
                  </span>
                </span>
                {s.archived && <span className="archived-ribbon"><span>已归档</span></span>}
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
                <span className="session-project-path-text">{shortPath(displayPath)}</span>
              </CopyButton>
            ) : null;
            return (
              <div className="session-card-row" key={item.key} data-index={item.index}
                ref={list.measureElement} style={{ top: item.start }}>
                {batchMode ? (
                  <label className={`${cardClass} selectable`} title={cardTitle}>
                    <Checkbox
                      checked={checkedSessionKeys.has(key)}
                      onChange={() => onSelect(s)}
                      disabled={disabled || busy || searchPending}
                      aria-label={`选择会话：${title}（${s.projectDir}）`}
                    />
                    <div className="session-card-content">
                      {body}
                      {pathButton}
                    </div>
                  </label>
                ) : (
                  <div className={cardClass} title={cardTitle}>
                    <button type="button" className="session-card-open" onClick={() => onSelect(s)} disabled={disabled}>
                      {body}
                    </button>
                    {agent && <SessionActions agent={agent} session={s} onRequestDelete={onRequestDelete}
                      onToggleArchive={onToggleArchive} toast={toast} compact disabled={disabled} />}
                    {pathButton}
                  </div>
                )}
              </div>
            );
          })}
          {sessions.length === 0 && (loading || !searchPending) && <div className="empty-hint" role="status">{emptyHint}</div>}
        </div>
      </div>
    </div>
  );
}
