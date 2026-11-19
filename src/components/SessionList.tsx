import { useCallback, useLayoutEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { AgentInfo, CatalogFilter, ConversationSearchHit, SessionDeleteFailure, SessionMeta } from "../types";
import { ChevronRight, Search, X } from "lucide-react";
import UiIcon from "./UiIcon";
import { formatSize, fullTime, relativeTime, sessionKey, shortPath } from "../utils";
import ModelLabel from "./ModelLabel";
import HighlightedText from "./HighlightedText";
import ReadDiagnostics from "./ReadDiagnostics";
import SessionActions from "./SessionActions";
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
  onToggleSession: (session: SessionMeta) => void;
  onToggleAll: () => void;
  onDeleteSelected: () => void;
  disabled: boolean;
  loading: boolean;
  refreshing: boolean;
  loadError: string | null;
  warnings: string[];
  onRetryLoad: () => void;
  progress: { completed: number; total: number } | null;
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
  onToggleSession,
  onToggleAll,
  onDeleteSelected,
  disabled,
  loading,
  refreshing,
  loadError,
  warnings,
  onRetryLoad,
  progress,
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
    getItemKey, estimateSize: () => 76, gap: 6, paddingStart: 8, paddingEnd: 8, overscan: 5 });
  useLayoutEffect(() => { scroll.current?.scrollTo(0, 0); }, [scopeKey, search]);
  const emptyHint = loading ? "正在读取会话…"
    : loadError || warnings.length > 0 ? "会话列表读取不完整。请重试。"
    : "没有匹配的会话";
  return (
    <div className="session-list" aria-busy={busy}>
      <div className="search-box">
        <div className="session-search-field">
          <UiIcon icon={Search} size={16} className="search-icon" />
          <input
            type="search"
            className="control-input"
            placeholder="搜索会话或正文…"
            aria-label="搜索会话或正文"
            maxLength={256}
            value={search}
            onChange={(e) => onSearch(e.target.value)}
            disabled={disabled}
          />
          {search && <button type="button" className="search-clear" aria-label="清除搜索" title="清除搜索" onClick={() => onSearch("")} disabled={disabled}>
            <UiIcon icon={X} />
          </button>}
        </div>
        {(searchPending || searchSkipped > 0 || searchError) && (
          <div className={`search-status${searchSkipped || searchError ? " warning" : ""}`} role="status" title={searchError ?? undefined}>
            {searchPending ? "正在搜索正文…" : searchError ? <>无法搜索正文 <button onClick={onRetrySearch} disabled={disabled}>重试</button></> : `无法搜索 ${searchSkipped} 个会话`}
          </div>
        )}
        {loadError && <ReadDiagnostics className="catalog-error" title="无法读取会话列表" errors={[loadError]} retry={onRetryLoad} busy={disabled || busy} />}
        {warnings.length > 0 && <ReadDiagnostics className="catalog-warnings" title={`${warnings.length} 条读取提示`} errors={warnings} description="部分会话未能读取。" retry={onRetryLoad} busy={disabled || busy} />}
        <div className="session-list-options">
          <div className="session-list-filters">
            <CheckboxField label="显示空会话" checked={filter.includeEmpty} disabled={disabled}
              onChange={includeEmpty => onFilterChange({ ...filter, includeEmpty })} />
            {agent?.supportsArchive && (
              <CheckboxField label="显示已归档" checked={filter.includeArchived} disabled={disabled}
                onChange={includeArchived => onFilterChange({ ...filter, includeArchived })} />
            )}
          </div>
          {agent && agent.deletePolicy !== "readOnly" && (
            <button onClick={onToggleBatch} disabled={disabled || busy || searchPending || (!batchMode && sessions.length === 0)}>
              {batchMode ? "完成" : "批量管理"}
            </button>
          )}
        </div>
      </div>
      {batchMode && (
        <div className="batch-toolbar">
          <div className="batch-status" role="status">
            {progress ? `正在删除会话（${progress.completed}/${progress.total}）` : `已选 ${selectedCount} 个会话`}
          </div>
          <div className="batch-actions">
            <button onClick={onToggleAll} disabled={disabled || busy || searchPending || sessions.length === 0}>
              {allSelected ? "取消全选" : "全选结果"}
            </button>
            <button className="danger" onClick={onDeleteSelected} disabled={disabled || busy || searchPending || selectedCount === 0}>
              删除所选
            </button>
          </div>
          <p className="batch-hint">更改筛选条件会取消选择。</p>
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
            const statsKnown = !s.statsPending;
            const isEmpty = statsKnown && s.realUserCount === 0;
            const key = sessionKey(s);
            const hit = searchHits.get(key);
            const checked = checkedSessionKeys.has(key);
            const title = s.title || "无标题";
            const hidePath = hiddenPathSessionKeys?.has(key) ?? false;
            const displayPath = displayRoots?.get(key) ?? s.cwd;
            const content = (
              <>
                <span className="session-card-title">{title}</span>
                {hit && <span className="session-search-snippet"><span>{hit.role === "user" ? "用户" : "助手"}</span><HighlightedText text={hit.snippet} query={search} /></span>}
                <span className="session-card-sub">
                  <span className="session-card-stats">
                    {statsKnown && <span>{s.realUserCount + s.assistantTextCount} 条消息</span>}
                    {s.sizeBytes != null && <span className="session-card-size">{formatSize(s.sizeBytes)}</span>}
                  </span>
                  <time dateTime={s.lastTs ?? undefined} title={fullTime(s.lastTs)}>{relativeTime(s.lastTs)}</time>
                </span>
                <span className="session-card-badges">
                  {s.readError && <span className="badge read-error-badge" title="打开会话可查看诊断或重试。">正文不可读</span>}
                  {isEmpty && <span className="badge empty-badge">空会话</span>}
                  <ModelLabel model={s.model} badge />
                  {showPaths && !hidePath && displayPath && <span className="badge dim session-project-path" title={s.cwd ?? displayPath}>{shortPath(displayPath)}</span>}
                </span>
                {s.archived && <span className="archived-ribbon"><span>已归档</span></span>}
              </>
            );
            return (
              <div className="session-card-row" key={item.key} data-index={item.index}
                ref={list.measureElement} style={{ top: item.start }}>
                {batchMode ? (
                  <label className={`session-card selectable${checked ? " active" : ""}${s.archived ? " archived" : ""}`} title={hidePath ? s.cwd ?? undefined : undefined}>
                    <Checkbox
                      checked={checked}
                      onChange={() => onToggleSession(s)}
                      disabled={disabled || busy || searchPending}
                      aria-label={`选择会话：${title}（${s.projectDir}）`}
                    />
                    <div className="session-card-content">{content}</div>
                  </label>
                ) : (
                  <div
                    title={hidePath ? s.cwd ?? undefined : undefined}
                    className={`session-card${selectedSessionKey === key ? " active" : ""}${isEmpty ? " empty" : ""}${s.archived ? " archived" : ""}`}
                  >
                    <button type="button" className="session-card-open" onClick={() => onSelect(s)} disabled={disabled}>
                      {content}
                    </button>
                    {agent && <SessionActions agent={agent} session={s} onRequestDelete={onRequestDelete}
                      onToggleArchive={onToggleArchive} toast={toast} compact disabled={disabled} />}
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
