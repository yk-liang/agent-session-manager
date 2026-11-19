import type { AgentInfo, ConversationSearchHit, SessionDeleteFailure, SessionMeta } from "../types";
import { ChevronRight, Search, X } from "lucide-react";
import UiIcon from "./UiIcon";
import { formatSize, relativeTime, sessionKey, shortPath } from "../utils";
import SessionModel from "./SessionModel";
import HighlightedText from "./HighlightedText";
import ReadDiagnostics from "./ReadDiagnostics";
import SessionActions from "./SessionActions";

interface Props {
  sessions: SessionMeta[];
  selectedSessionKey: string | null;
  search: string;
  onSearch: (s: string) => void;
  onSelect: (s: SessionMeta) => void;
  showEmpty: boolean;
  onShowEmpty: (v: boolean) => void;
  batchMode: boolean;
  agent?: AgentInfo;
  onRequestDelete: (session: SessionMeta) => void;
  toast: (message: string, error?: boolean) => void;
  checkedSessionKeys: Set<string>;
  onToggleBatch: () => void;
  onToggleSession: (session: SessionMeta) => void;
  onToggleAll: () => void;
  onDeleteSelected: () => void;
  disabled: boolean;
  loading: boolean;
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
  sessions,
  selectedSessionKey,
  search,
  onSearch,
  onSelect,
  showEmpty,
  onShowEmpty,
  batchMode,
  agent,
  onRequestDelete,
  toast,
  checkedSessionKeys,
  onToggleBatch,
  onToggleSession,
  onToggleAll,
  onDeleteSelected,
  disabled,
  loading,
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
  const selectedCount = new Set(sessions.filter((s) => checkedSessionKeys.has(sessionKey(s))).map(sessionKey)).size;
  const allSelected = sessions.length > 0 && sessions.every((s) => checkedSessionKeys.has(sessionKey(s)));
  const emptyHint = loading ? "正在读取会话…"
    : loadError || warnings.length > 0 ? "会话列表读取不完整。请重试。"
    : "没有匹配的会话";
  return (
    <div className="session-list" aria-busy={loading}>
      <div className="search-box">
        <div className="session-search-field">
          <UiIcon icon={Search} size={16} className="search-icon" />
          <input
            type="search"
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
        {loadError && <ReadDiagnostics className="catalog-error" title="无法读取会话列表" errors={[loadError]} retry={onRetryLoad} busy={disabled || loading} />}
        {warnings.length > 0 && <ReadDiagnostics className="catalog-warnings" title={`${warnings.length} 条读取提示`} errors={warnings} description="部分会话未能读取。" retry={onRetryLoad} busy={disabled || loading} />}
        <div className="session-list-options">
          <label className="toggle show-empty-toggle">
            <input
              type="checkbox"
              checked={showEmpty}
              onChange={(e) => onShowEmpty(e.target.checked)}
              disabled={disabled}
            />
            显示空会话
          </label>
          {agent && agent.deletePolicy !== "readOnly" && (
            <button onClick={onToggleBatch} disabled={disabled || loading || searchPending || (!batchMode && sessions.length === 0)}>
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
            <button onClick={onToggleAll} disabled={disabled || loading || searchPending || sessions.length === 0}>
              {allSelected ? "取消全选" : "全选结果"}
            </button>
            <button className="danger" onClick={onDeleteSelected} disabled={disabled || loading || searchPending || selectedCount === 0}>
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
                <div>{shortPath(session.cwd) || session.projectDir} · {session.sessionId}</div>
                <div>{error}</div>
              </li>
            ))}
          </ul>
        </details>
      )}
      <div className="session-cards">
        {sessions.map((s) => {
          const isEmpty = !s.statsPending && s.realUserCount === 0;
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
                {relativeTime(s.lastTs)} · {!s.statsPending && <>{s.realUserCount + s.assistantTextCount} 条消息 · </>}
                {formatSize(s.sizeBytes)}
              </span>
              <span className="session-card-badges">
                {s.readError && <span className="badge read-error-badge" title="打开会话可查看诊断或重试。">正文不可读</span>}
                {isEmpty && <span className="badge empty-badge">空会话</span>}
                <SessionModel session={s} badge />
                {showPaths && !hidePath && displayPath && <span className="badge dim session-project-path" title={s.cwd ?? displayPath}>{shortPath(displayPath)}</span>}
              </span>
            </>
          );
          return batchMode ? (
            <label key={key} className={`session-card selectable ${checked ? "active" : ""}`} title={hidePath ? s.cwd ?? undefined : undefined}>
              <input
                type="checkbox"
                checked={checked}
                onChange={() => onToggleSession(s)}
                disabled={disabled || loading || searchPending}
                aria-label={`选择会话：${title}（${s.projectDir}）`}
              />
              <div className="session-card-content">{content}</div>
            </label>
          ) : (
            <div key={key}
              title={hidePath ? s.cwd ?? undefined : undefined}
              className={`session-card ${selectedSessionKey === key ? "active" : ""} ${isEmpty ? "empty" : ""}`}
            >
              <button type="button" className="session-card-open" onClick={() => onSelect(s)} disabled={disabled}>
                {content}
              </button>
              {agent && <SessionActions agent={agent} session={s} onRequestDelete={onRequestDelete} toast={toast} compact disabled={disabled} />}
            </div>
          );
        })}
        {sessions.length === 0 && !searchPending && <div className="empty-hint">{emptyHint}</div>}
      </div>
    </div>
  );
}
