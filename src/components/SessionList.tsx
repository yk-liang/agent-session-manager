import type { ConversationSearchHit, SessionDeleteFailure, SessionMeta } from "../types";
import { ChevronRight, Search, X } from "lucide-react";
import UiIcon from "./UiIcon";
import { formatSize, relativeTime, sessionKey, shortPath } from "../utils";
import SessionModel from "./SessionModel";
import HighlightedText from "./HighlightedText";

interface Props {
  sessions: SessionMeta[];
  selectedKey: string | null;
  search: string;
  onSearch: (s: string) => void;
  onSelect: (s: SessionMeta) => void;
  showEmpty: boolean;
  onShowEmpty: (v: boolean) => void;
  batchMode: boolean;
  checkedKeys: Set<string>;
  onToggleBatch: () => void;
  onToggleSession: (session: SessionMeta) => void;
  onToggleAll: () => void;
  onDeleteSelected: () => void;
  deleting: boolean;
  loading: boolean;
  progress: { completed: number; total: number } | null;
  failures: SessionDeleteFailure[];
  searchHits: Map<string, ConversationSearchHit>;
  searchPending: boolean;
  searchSkipped: number;
  searchError: string | null;
  showPaths: boolean;
  hiddenPathSessionKeys?: ReadonlySet<string>;
  displayRoots?: ReadonlyMap<string, string | null>;
}

export default function SessionList({
  sessions,
  selectedKey,
  search,
  onSearch,
  onSelect,
  showEmpty,
  onShowEmpty,
  batchMode,
  checkedKeys,
  onToggleBatch,
  onToggleSession,
  onToggleAll,
  onDeleteSelected,
  deleting,
  loading,
  progress,
  failures,
  searchHits,
  searchPending,
  searchSkipped,
  searchError,
  showPaths,
  hiddenPathSessionKeys,
  displayRoots,
}: Props) {
  const selectedCount = new Set(sessions.filter((s) => checkedKeys.has(sessionKey(s))).map(sessionKey)).size;
  const allSelected = sessions.length > 0 && sessions.every((s) => checkedKeys.has(sessionKey(s)));
  return (
    <div className="session-list">
      <div className="search-box">
        <div className="session-search-field">
          <UiIcon icon={Search} size={16} className="search-icon" />
          <input
            type="search"
            placeholder="搜索会话或对话内容…"
            aria-label="搜索会话或对话内容"
            maxLength={256}
            value={search}
            onChange={(e) => onSearch(e.target.value)}
            disabled={deleting}
          />
          {search && <button type="button" className="search-clear" aria-label="清除搜索" title="清除搜索" onClick={() => onSearch("")} disabled={deleting}>
            <UiIcon icon={X} />
          </button>}
        </div>
        {(searchPending || searchSkipped > 0 || searchError) && (
          <div className={`search-status${searchSkipped || searchError ? " warning" : ""}`} role="status" title={searchError ?? undefined}>
            {searchPending ? "正在搜索对话…" : searchError ? "正文搜索失败，可刷新重试" : `${searchSkipped} 个会话未能搜索`}
          </div>
        )}
        <div className="session-list-options">
          <label className="toggle show-empty-toggle">
            <input
              type="checkbox"
              checked={showEmpty}
              onChange={(e) => onShowEmpty(e.target.checked)}
              disabled={deleting}
            />
            显示空会话
          </label>
          <button onClick={onToggleBatch} disabled={deleting || loading || searchPending || (!batchMode && sessions.length === 0)}>
            {batchMode ? "完成" : "批量管理"}
          </button>
        </div>
      </div>
      {batchMode && (
        <div className="batch-toolbar">
          <div className="batch-status" role="status">
            {progress ? `正在删除 ${progress.completed} / ${progress.total}，请勿关闭应用` : `已选 ${selectedCount} 个会话`}
          </div>
          <div className="batch-actions">
            <button onClick={onToggleAll} disabled={deleting || loading || searchPending || sessions.length === 0}>
              {allSelected ? "取消全选" : "全选当前结果"}
            </button>
            <button className="danger" onClick={onDeleteSelected} disabled={deleting || loading || searchPending || selectedCount === 0}>
              删除所选
            </button>
          </div>
          <p className="batch-hint">更改筛选条件会清空勾选。</p>
        </div>
      )}
      {failures.length > 0 && (
        <details className="batch-failures" open>
          <summary><UiIcon icon={ChevronRight} className="details-caret" />{failures.length} 个会话删除失败</summary>
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
          const checked = checkedKeys.has(key);
          const title = s.title || (!s.hasTranscript ? "(空壳目录)" : "(无标题)");
          const hidePath = hiddenPathSessionKeys?.has(key) ?? false;
          const displayPath = displayRoots?.get(key) ?? s.cwd;
          const content = (
            <>
              <div className="session-card-title">{title}</div>
              {hit && <div className="session-search-snippet"><span>{hit.role === "user" ? "用户" : "助手"}</span><HighlightedText text={hit.snippet} query={search} /></div>}
              <div className="session-card-sub">
                {relativeTime(s.lastTs)} · {!s.statsPending && <>{s.realUserCount + s.assistantTextCount} 条对话 · </>}
                {formatSize(s.sizeBytes)}
              </div>
              <div className="session-card-badges">
                {isEmpty && <span className="badge empty-badge">空会话</span>}
                <SessionModel session={s} badge />
                {showPaths && !hidePath && displayPath && <span className="badge dim session-project-path" title={s.cwd ?? displayPath}>{shortPath(displayPath)}</span>}
              </div>
            </>
          );
          return batchMode ? (
            <label key={key} className={`session-card selectable ${checked ? "active" : ""}`} title={hidePath ? s.cwd ?? undefined : undefined}>
              <input
                type="checkbox"
                checked={checked}
                onChange={() => onToggleSession(s)}
                disabled={deleting || loading || searchPending}
                aria-label={`选择会话：${title}（${s.projectDir}）`}
              />
              <div className="session-card-content">{content}</div>
            </label>
          ) : (
            <button
              key={key}
              title={hidePath ? s.cwd ?? undefined : undefined}
              className={`session-card ${selectedKey === key ? "active" : ""} ${isEmpty ? "empty" : ""}`}
              onClick={() => onSelect(s)}
              disabled={deleting}
            >
              {content}
            </button>
          );
        })}
        {sessions.length === 0 && !searchPending && <div className="empty-hint">没有匹配的会话</div>}
      </div>
    </div>
  );
}
