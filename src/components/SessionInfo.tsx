import { useId, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import CopyButton from "./CopyButton";
import UiIcon from "./UiIcon";
import type { SessionMeta } from "../types";
import ModelLabel from "./ModelLabel";
import useDismiss from "./useDismiss";
import { formatClient } from "../utils";

export default function SessionInfo({ meta, agentName, models = [], onCopyError }: {
  meta: SessionMeta;
  agentName: string;
  models?: string[];
  onCopyError: (error: unknown) => void;
}) {
  const currentModel = meta.model || models[0];
  const otherModels = models.filter((model) => model !== currentModel);
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const id = useId();

  useDismiss(open, [root], () => setOpen(false));

  return (
    <div className="session-info" ref={root}>
      <button
        type="button"
        className="session-info-toggle btn-sm btn-ghost"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen(!open)}
      >
        更多信息
        <UiIcon icon={ChevronDown} size={12} className={open ? "expanded" : ""} />
      </button>
      {open && (
        <div
          className="session-info-popover"
          id={id}
          role="group"
          aria-label="会话信息"
        >
          <dl>
            {currentModel && (
              <div>
                <dt>{otherModels.length ? "最近使用的模型" : "模型"}</dt>
                <dd>
                  <ModelLabel model={currentModel} />
                </dd>
              </div>
            )}
            {otherModels.length > 0 && <div className="session-model-history">
              <dt>其他模型</dt>
              <dd>{otherModels.map((model) => <ModelLabel key={model} model={model} />)}</dd>
            </div>}
            {meta.cwd && (
              <div>
                <dt>工作目录</dt>
                <dd className="session-info-copy">
                  <span>{meta.cwd}</span>
                  <CopyButton className="btn-icon btn-ghost" value={meta.cwd} label="复制工作目录" onError={onCopyError} />
                </dd>
              </div>
            )}
            <div>
              <dt>客户端</dt>
              <dd>{formatClient(agentName, meta.client, meta.version).label}</dd>
            </div>
            {meta.codex?.source && <div><dt>会话来源</dt><dd><code>{meta.codex.source}</code></dd></div>}
            {meta.codex?.threadSource && <div><dt>线程类型</dt><dd><code>{meta.codex.threadSource}</code></dd></div>}
            {meta.gitBranch && (
              <div>
                <dt>Git 分支</dt>
                <dd>{meta.gitBranch}</dd>
              </div>
            )}
          </dl>
        </div>
      )}
    </div>
  );
}
