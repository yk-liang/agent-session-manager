import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import UiIcon from "./UiIcon";
import type { ConfigValidation, WorkspaceFile } from "../types";
import * as api from "../api";

interface FileProps {
  file: WorkspaceFile;
  agent: string;
  disabled: boolean;
  onSaved: (key: string, content: string) => Promise<void>;
  onDirtyChange: (key: string, dirty: boolean) => void;
  onSavingChange: (saving: boolean) => void;
}

export default function ConfigFileEditor({
  file,
  agent,
  disabled,
  onSaved,
  onDirtyChange,
  onSavingChange,
}: FileProps) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  // 编辑开始时冻结基线。其他文件保存触发的重新读取，不得替换这份草稿的版本。
  const baseline = useRef({ content: file.content, revision: file.revision });
  const [validation, setValidation] = useState<ConfigValidation>(
    file.validation,
  );
  const [checking, setChecking] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const validationGen = useRef(0);
  const gutterRef = useRef<HTMLPreElement>(null);
  const dirty =
    editing &&
    (baseline.current.content === null || draft !== baseline.current.content);
  const text = editing ? draft : (file.content ?? "");
  const lineCount = text.split("\n").length;

  useEffect(() => {
    if (!editing) return;
    const gen = ++validationGen.current;
    setChecking(true);
    const timer = window.setTimeout(() => {
      api
        .validateConfigContent(agent, file.key, draft)
        .then((result) => {
          if (gen === validationGen.current) setValidation(result);
        })
        .catch((e) => {
          if (gen === validationGen.current)
            setValidation({
              valid: false,
              error: `暂时无法校验：${String(e)}`,
            });
        })
        .finally(() => {
          if (gen === validationGen.current) setChecking(false);
        });
    }, 300);
    return () => {
      window.clearTimeout(timer);
      validationGen.current++;
    };
  }, [agent, file.key, editing, draft]);

  useEffect(
    () => () => onDirtyChange(file.key, false),
    [file.key, onDirtyChange],
  );

  const startEdit = () => {
    baseline.current = { content: file.content, revision: file.revision };
    const initial =
      file.content ??
      (file.key.endsWith(".json") || file.key.endsWith(".jsonc") ? "{}\n" : "");
    setDraft(initial);
    setEditing(true);
    setOpen(true);
    setError(null);
    onDirtyChange(file.key, file.content === null);
  };

  const cancelEdit = () => {
    setEditing(false);
    setError(null);
    onDirtyChange(file.key, false);
  };

  const saveEdit = async () => {
    if (disabled || saving || checking || !dirty || !validation.valid) return;
    setSaving(true);
    onSavingChange(true);
    setError(null);
    try {
      await api.updateConfigFile(
        agent,
        file.key,
        draft,
        baseline.current.revision,
      );
      onDirtyChange(file.key, false);
      setEditing(false);
      await onSaved(file.key, draft);
    } catch (e) {
      // 拒绝覆盖时保留完整草稿，用户可复制后重新读取磁盘内容。
      setError(String(e));
    } finally {
      setSaving(false);
      onSavingChange(false);
    }
  };

  const displayedValidation = editing ? validation : file.validation;
  return (
    <article
      className={`config-file${dirty ? " is-dirty" : ""}`}
      data-file-key={file.key}
    >
      <div className="config-file-head">
        <button
          className="collapsible-head"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          title={file.path}
        >
          <UiIcon icon={open ? ChevronDown : ChevronRight} className="caret" />
          <strong>{file.label}</strong>
          {!file.present && (
            <span className="config-badge muted">尚未创建</span>
          )}
          {dirty && <span className="config-badge amber">未保存</span>}
          <span className="config-line-count">
            {file.present || editing ? `${lineCount} 行` : ""}
          </span>
        </button>
        {!editing && (
          <button className="edit-btn" onClick={startEdit} disabled={disabled}>
            {file.present ? "编辑" : "创建"}
          </button>
        )}
      </div>
      {open && (
        <div>
          <div className="config-file-path">{file.path}</div>
          <div
            className="config-code"
            style={{ "--config-lines": lineCount } as React.CSSProperties}
          >
            <pre
              className="config-line-numbers"
              ref={gutterRef}
              aria-hidden="true"
            >
              {Array.from({ length: lineCount }, (_, i) => i + 1).join("\n")}
            </pre>
            {editing ? (
              <textarea
                aria-label={`${file.label} 配置内容`}
                value={draft}
                wrap="off"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoFocus
                disabled={disabled || saving}
                onChange={(e) => {
                  const value = e.target.value;
                  setDraft(value);
                  onDirtyChange(
                    file.key,
                    baseline.current.content === null ||
                      value !== baseline.current.content,
                  );
                  setError(null);
                }}
                onScroll={(e) => {
                  if (gutterRef.current)
                    gutterRef.current.scrollTop = e.currentTarget.scrollTop;
                }}
                onKeyDown={(e) => {
                  if ((e.metaKey || e.ctrlKey) && e.key === "s") {
                    e.preventDefault();
                    void saveEdit();
                  }
                }}
              />
            ) : (
              <pre
                className="code config-json"
                tabIndex={0}
                aria-label={`${file.label} 配置内容（只读）`}
                onScroll={(e) => {
                  if (gutterRef.current)
                    gutterRef.current.scrollTop = e.currentTarget.scrollTop;
                }}
              >
                {text || "尚无配置内容"}
              </pre>
            )}
          </div>
          {(editing || !displayedValidation.valid) && (
            <div className="config-editor-footer">
              <span
                className={
                  !displayedValidation.valid
                    ? "validation-error"
                    : "validation-ok"
                }
                role="status"
              >
                {editing && checking
                  ? "校验中…"
                  : displayedValidation.valid
                    ? file.key.endsWith(".json") ||
                      file.key.endsWith(".jsonc") ||
                      file.key.endsWith(".toml")
                      ? "已校验"
                      : "文本"
                    : displayedValidation.error}
              </span>
              {editing && (
                <div className="config-edit-buttons">
                  <button onClick={cancelEdit} disabled={saving}>
                    取消编辑
                  </button>
                  <button
                    className="primary"
                    onClick={saveEdit}
                    disabled={
                      disabled ||
                      saving ||
                      checking ||
                      !dirty ||
                      !validation.valid
                    }
                  >
                    {saving ? "保存中…" : "保存"}
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      )}
      {error && (
        <div className="config-notice error" role="alert">
          {error}
        </div>
      )}
      {!open && !file.validation.valid && (
        <div className="config-notice error" role="alert">
          {file.validation.error}
        </div>
      )}
    </article>
  );
}
