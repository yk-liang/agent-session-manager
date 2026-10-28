import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, Trash2, X } from "lucide-react";
import UiIcon from "./UiIcon";
import ConfirmDialog from "./ConfirmDialog";
import FileDiff from "./ConfigFileDiff";
import FileEditor from "./ConfigFileEditor";
import HelpTip from "./HelpTip";
import AgentIcon from "./AgentIcon";
import type {
  ConfigActivity,
  ConfigChangeKind,
  ConfigChangePreview,
  ConfigWorkspace,
  ProfileSummary,
} from "../types";
import * as api from "../api";

interface Props {
  agent: string;
  displayName: string;
  refreshKey: number;
  toast: (msg: string, isError?: boolean) => void;
  onActivityChange: (activity: ConfigActivity) => void;
}

const dateLabel = (value: number) => new Date(value).toLocaleString("zh-CN");

export default function ConfigPanel({
  agent,
  displayName,
  refreshKey,
  toast,
  onActivityChange,
}: Props) {
  const [workspace, setWorkspace] = useState<ConfigWorkspace | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [fileSaveCount, setFileSaveCount] = useState(0);
  const [dirtyKeys, setDirtyKeys] = useState<Set<string>>(new Set());
  const [editorEpoch, setEditorEpoch] = useState(0);
  const [name, setName] = useState("");
  const [saveOpen, setSaveOpen] = useState(false);
  const [allBackups, setAllBackups] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [preview, setPreview] = useState<ConfigChangePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProfileSummary | null>(null);
  const [notice, setNotice] = useState<{
    text: string;
    error?: boolean;
  } | null>(null);
  const reloadGen = useRef(0);
  const working = busy || loading || fileSaveCount > 0;
  // 读取可以取消或切页；仅写入事务和正在确认的操作需要阻止离开。
  const navigationBusy = busy || fileSaveCount > 0;
  const dirty = dirtyKeys.size > 0;
  const disabled = working || loadFailed;

  const onDirtyChange = useCallback((key: string, changed: boolean) => {
    setDirtyKeys((current) => {
      if (current.has(key) === changed) return current;
      const next = new Set(current);
      if (changed) next.add(key);
      else next.delete(key);
      return next;
    });
  }, []);
  const onSavingChange = useCallback(
    (saving: boolean) =>
      setFileSaveCount((n) => Math.max(0, n + (saving ? 1 : -1))),
    [],
  );

  useEffect(() => {
    onActivityChange({ dirty, busy: navigationBusy });
  }, [dirty, navigationBusy, onActivityChange]);
  useEffect(
    () => () => onActivityChange({ dirty: false, busy: false }),
    [onActivityChange],
  );

  const reload = useCallback(
    async (resetEditors = false) => {
      const gen = ++reloadGen.current;
      setLoading(true);
      try {
        const result = await api.getConfigWorkspace(agent);
        if (gen !== reloadGen.current) return false;
        setWorkspace(result);
        setLoadFailed(false);
        if (resetEditors) {
          setEditorEpoch((n) => n + 1);
          setDirtyKeys(new Set());
        }
        return true;
      } catch (e) {
        if (gen === reloadGen.current) {
          setLoadFailed(true);
          setNotice({ text: `无法刷新配置：${String(e)}`, error: true });
        }
        return false;
      } finally {
        if (gen === reloadGen.current) setLoading(false);
      }
    },
    [agent],
  );

  useEffect(() => {
    void reload(true);
    return () => {
      reloadGen.current++;
    };
  }, [reload, refreshKey]);

  const onFileSaved = useCallback(
    async (key: string, content: string) => {
      // 写入已确认，刷新失败也展示已保存内容；进一步修改需先成功重新读取版本。
      setWorkspace(
        (current) =>
          current && {
            ...current,
            files: current.files.map((f) =>
              f.key === key
                ? {
                    ...f,
                    content,
                    present: true,
                    validation: { valid: true, error: null },
                  }
                : f,
            ),
          },
      );
      const refreshed = await reload();
      setNotice(
        refreshed
          ? null
          : {
              text: `${key} 已保存，但无法重新读取状态；请刷新后重试。`,
              error: true,
            },
      );
      toast(`${key} 已保存`);
    },
    [reload, toast],
  );

  const save = async () => {
    if (!workspace || !name.trim() || disabled || dirty) return;
    setBusy(true);
    try {
      await api.saveProfile(agent, name.trim(), workspace.revision);
      setNotice(null);
      toast(`方案「${name.trim()}」已保存`);
      setName("");
      setSaveOpen(false);
      await reload();
    } catch (e) {
      setNotice({ text: String(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  const startPreview = async (kind: ConfigChangeKind, target: string) => {
    if (working || dirty) return;
    setBusy(true);
    setPreviewError(null);
    try {
      setPreview(await api.previewConfigChange(agent, kind, target));
    } catch (e) {
      if (preview) setPreviewError(String(e));
      else setNotice({ text: String(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    if (!preview || working || dirty || previewError) return;
    setBusy(true);
    try {
      await api.applyConfigChange(agent, preview);
      setPreview(null);
      const refreshed = await reload(true);
      setNotice(
        refreshed
          ? null
          : { text: "已保存，状态刷新失败；请刷新后重试。", error: true },
      );
      toast(preview.kind === "profile" ? "配置已应用" : "备份已恢复");
    } catch (e) {
      setPreviewError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!deleteTarget || working) return;
    setBusy(true);
    try {
      await api.deleteProfile(agent, deleteTarget.name);
      setDeleteTarget(null);
      await reload();
    } catch (e) {
      setNotice({ text: String(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (!preview) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) setPreview(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [preview, busy]);

  if (!workspace)
    return (
      <div className="config-panel">
        <div className="config-page-heading">
          <div>
            <h1>
              <AgentIcon agent={agent} />
              {displayName}
            </h1>
          </div>
        </div>
        {notice && (
          <div role="alert" className="config-notice error">
            {notice.text}
          </div>
        )}
        <p className="hint">
          {loading ? "加载中…" : "读取失败，请使用顶部刷新重试。"}
        </p>
      </div>
    );

  const matching = workspace.matchingProfiles;
  const validFiles = workspace.files.every((f) => f.validation.valid);
  const changedFiles =
    preview?.files.filter((f) => f.action !== "unchanged") ?? [];
  const backups = allBackups
    ? workspace.backups
    : workspace.backups.slice(0, 5);
  const hasOverview =
    workspace.overview.model ||
    workspace.overview.provider ||
    workspace.overview.endpoint ||
    workspace.overview.mcpServers.length > 0;

  return (
    <div className="config-panel">
      <header className="config-page-heading">
        <div>
          <h1>
            <AgentIcon agent={agent} />
            {displayName}
            {workspace.profiles.length > 0 && (
              <span
                className={`config-status ${matching.length ? "matched" : ""}`}
                title={
                  matching.length
                    ? "当前文件与这些方案一致"
                    : "当前文件未匹配已保存的方案"
                }
              >
                {matching.length ? matching.join("、") : "自定义"}
              </span>
            )}
          </h1>
        </div>
        <div className="config-page-actions">
          <button
            className="primary"
            disabled={
              disabled ||
              dirty ||
              !validFiles ||
              !workspace.files.some((f) => f.present)
            }
            onClick={() => setSaveOpen(!saveOpen)}
          >
            保存为方案
          </button>
        </div>
      </header>

      {hasOverview && (
        <div className="config-overview" aria-label="当前配置摘要">
          {workspace.overview.model && (
            <div>
              <span>模型</span>
              <strong title={workspace.overview.model}>
                {workspace.overview.model}
              </strong>
            </div>
          )}
          {workspace.overview.provider && (
            <div>
              <span>提供商</span>
              <strong>{workspace.overview.provider}</strong>
            </div>
          )}
          {workspace.overview.endpoint && (
            <div>
              <span>服务地址</span>
              <strong title={workspace.overview.endpoint}>
                {workspace.overview.endpoint}
              </strong>
            </div>
          )}
          {workspace.overview.mcpServers.length > 0 && (
            <div>
              <span>MCP</span>
              <strong title={workspace.overview.mcpServers.join("、")}>
                {workspace.overview.mcpServers.length} 个
              </strong>
            </div>
          )}
        </div>
      )}

      {dirty && (
        <div className="config-notice warning" role="status">
          {dirtyKeys.size} 个文件未保存
        </div>
      )}
      {notice && (
        <div
          className={`config-notice ${notice.error ? "error" : "success"}`}
          role={notice.error ? "alert" : "status"}
        >
          {notice.text}
          <button
            className="notice-dismiss"
            aria-label="关闭提示"
            onClick={() => setNotice(null)}
          >
            <UiIcon icon={X} />
          </button>
        </div>
      )}
      {workspace.warnings.length > 0 && (
        <details className="config-warnings">
          <summary><UiIcon icon={ChevronRight} className="details-caret" />{workspace.warnings.length} 条读取提示</summary>
          {workspace.warnings.map((warning, i) => (
            <p key={i}>{warning}</p>
          ))}
        </details>
      )}

      <section className="config-section" aria-labelledby="profiles-title">
        <div className="config-section-heading">
          <div className="config-heading-label">
            <h2 id="profiles-title">配置方案</h2>
            <HelpTip label="配置方案">
              将当前用户配置保存为方案，切换前可查看差异。项目设置或启动参数可能覆盖这些配置，修改后
              CLI 可能需要重新加载。
            </HelpTip>
          </div>
          {workspace.profiles.length === 0 && !saveOpen && (
            <span className="config-empty-label">暂无方案</span>
          )}
        </div>
        {saveOpen && (
          <form
            className="config-save-form"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <input
              autoFocus
              placeholder="方案名称，如：工作 / 个人 / 测试供应商"
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={disabled}
              aria-label="方案名称"
            />
            <button
              className="primary"
              type="submit"
              disabled={disabled || dirty || !validFiles || !name.trim()}
            >
              保存方案
            </button>
            <button
              type="button"
              onClick={() => setSaveOpen(false)}
              disabled={working}
            >
              取消
            </button>
          </form>
        )}
        {workspace.profiles.length > 0 && (
          <div className="profile-list">
            {workspace.profiles.map((profile) => (
              <article
                key={profile.name}
                className={`profile-card${matching.includes(profile.name) ? " matched" : ""}`}
                title={profile.files
                  .filter((file) => file.present)
                  .map((file) => file.label)
                  .join(" · ")}
              >
                <div className="profile-info">
                  <div className="profile-name">
                    {profile.name}
                    {matching.includes(profile.name) && (
                      <span className="config-badge green">当前</span>
                    )}
                  </div>
                  <div className="profile-meta">
                    {dateLabel(profile.createdAt)}
                  </div>
                </div>
                <div className="profile-actions">
                  <button
                    className="primary"
                    disabled={disabled || dirty}
                    onClick={() => void startPreview("profile", profile.name)}
                  >
                    应用
                  </button>
                  <button
                    className="text-button"
                    aria-label={`删除方案 ${profile.name}`}
                    title="删除方案"
                    disabled={disabled}
                    onClick={() => setDeleteTarget(profile)}
                  >
                    <UiIcon icon={Trash2} />
                  </button>
                </div>
              </article>
            ))}
          </div>
        )}
      </section>

      <section className="config-section" aria-labelledby="files-title">
        <div className="config-section-heading">
          <div className="config-heading-label">
            <h2 id="files-title">配置文件</h2>
            <HelpTip label="配置文件">
              编辑后保存。存在语法错误或检测到外部修改时，会保留草稿并提示。
            </HelpTip>
          </div>
          {!validFiles && <span className="config-badge amber">格式有误</span>}
        </div>
        {workspace.files.length === 0 && (
          <div className="config-empty">暂无配置文件</div>
        )}
        {workspace.files.map((file) => (
          <FileEditor
            key={`${editorEpoch}:${file.key}`}
            file={file}
            agent={agent}
            disabled={disabled}
            onSaved={onFileSaved}
            onDirtyChange={onDirtyChange}
            onSavingChange={onSavingChange}
          />
        ))}
      </section>

      <section
        className="config-section config-history"
        aria-labelledby="backups-title"
      >
        <div className="config-section-heading">
          <div className="config-heading-label">
            <h2 id="backups-title">
              <button
                className="history-toggle"
                disabled={workspace.backups.length === 0}
                aria-expanded={historyOpen}
                aria-controls="config-backup-list"
                onClick={() => setHistoryOpen(!historyOpen)}
              >
                备份
                {workspace.backups.length > 0 && (
                  <UiIcon icon={historyOpen ? ChevronDown : ChevronRight} className="history-caret" />
                )}
              </button>
            </h2>
            <HelpTip label="备份" above>
              修改前自动备份，恢复前可查看差异。可恢复一次操作前的全部文件，最多显示最近
              100 条。
            </HelpTip>
          </div>
          {workspace.backups.length === 0 && (
            <span className="config-empty-label">暂无备份</span>
          )}
        </div>
        {historyOpen && backups.length > 0 && (
          <div className="backup-list" id="config-backup-list">
            {backups.map((backup) => (
              <article className="backup-row" key={backup.id}>
                <div className="backup-marker" />
                <div className="backup-info">
                  <strong>{backup.label}</strong>
                  <p>
                    {dateLabel(backup.createdAt)} · {backup.files.join("、")}
                    {backup.legacy ? " · 单文件" : ""}
                  </p>
                </div>
                <button
                  disabled={disabled || dirty}
                  onClick={() => void startPreview("backup", backup.id)}
                >
                  恢复
                </button>
              </article>
            ))}
          </div>
        )}
        {historyOpen && workspace.backups.length > 5 && (
          <button
            className="text-button history-more"
            onClick={() => setAllBackups(!allBackups)}
          >
            {allBackups
              ? "收起历史记录"
              : `查看其余 ${workspace.backups.length - 5} 条记录`}
          </button>
        )}
      </section>

      {deleteTarget && (
        <ConfirmDialog
          title="删除配置方案"
          message={
            <>
              删除方案「{deleteTarget.name}
              」？已写入磁盘的文件和备份会保留，方案记录删除后不可恢复。
            </>
          }
          confirmLabel="删除方案"
          onConfirm={() => void remove()}
          onCancel={() => {
            if (!working) setDeleteTarget(null);
          }}
        />
      )}

      {preview && (
        <div
          className="modal-overlay"
          onClick={() => {
            if (!busy) setPreview(null);
          }}
        >
          <div
            className="modal config-change-modal"
            role="dialog"
            aria-modal="true"
            aria-label={preview.title}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="config-change-heading">
              <div>
                <h2>{preview.title}</h2>
              </div>
              <button
                aria-label="关闭差异预览"
                disabled={busy}
                onClick={() => setPreview(null)}
              >
                <UiIcon icon={X} />
              </button>
            </div>
            <p className="hint">
              {changedFiles.length
                ? `${changedFiles.length} 个文件将更改`
                : "内容一致，无需更改"}
            </p>
            {previewError && (
              <div className="config-notice error" role="alert">
                {previewError}
                <button
                  disabled={busy}
                  onClick={() =>
                    void startPreview(preview.kind, preview.target)
                  }
                >
                  重新读取差异
                </button>
              </div>
            )}
            <div className="config-change-files">
              {preview.files.map((file) => (
                <FileDiff key={file.key} file={file} />
              ))}
            </div>
            <div className="modal-actions">
              <button disabled={busy} onClick={() => setPreview(null)}>
                取消
              </button>
              <button
                className="primary"
                disabled={
                  working || !!previewError || changedFiles.length === 0
                }
                onClick={() => void apply()}
              >
                {busy
                  ? "处理中…"
                  : preview.kind === "profile"
                    ? "确认应用"
                    : "确认恢复"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
