import { useId, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import UiIcon from "./UiIcon";
import type { SessionMeta } from "../types";
import ModelLabel from "./ModelLabel";
import useDismiss from "./useDismiss";
import { clientLabel } from "../utils";

export default function SessionInfo({ meta, agentName, models = [] }: {
  meta: SessionMeta;
  agentName: string;
  models?: string[];
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
                <dd>{meta.cwd}</dd>
              </div>
            )}
            <div>
              <dt>客户端</dt>
              <dd>{clientLabel(agentName, meta.client, meta.version)}</dd>
            </div>
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
