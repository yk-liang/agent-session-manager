import { useEffect, useId, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import UiIcon from "./UiIcon";
import type { SessionMeta } from "../types";
import SessionModel, { ModelLabel } from "./SessionModel";
import type { HistoricalModel } from "../sessionModels";

export default function SessionInfo({ meta, models = [] }: { meta: SessionMeta; models?: HistoricalModel[] }) {
  const others = models.filter((model) => model.id !== meta.model);
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const id = useId();
  const hasInfo = !!(meta.model || meta.cwd || meta.version || meta.gitBranch);

  useEffect(() => {
    if (!open || !hasInfo) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape, true);
    };
  }, [open, hasInfo]);

  if (!hasInfo) return null;
  return (
    <div className="session-info" ref={root}>
      <button
        className="session-info-toggle"
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
            {meta.model && (
              <div>
                <dt>{others.length ? "最近模型" : "模型"}</dt>
                <dd>
                  <SessionModel session={meta} />
                </dd>
              </div>
            )}
            {others.length > 0 && <div className="session-model-history">
              <dt>其他模型</dt>
              <dd>{others.map((model) => <ModelLabel key={model.id} id={model.id} name={model.name} />)}</dd>
            </div>}
            {meta.cwd && (
              <div>
                <dt>路径</dt>
                <dd>{meta.cwd}</dd>
              </div>
            )}
            {meta.version && (
              <div>
                <dt>CLI 版本</dt>
                <dd>{meta.version}</dd>
              </div>
            )}
            {meta.gitBranch && (
              <div>
                <dt>分支</dt>
                <dd>{meta.gitBranch}</dd>
              </div>
            )}
          </dl>
        </div>
      )}
    </div>
  );
}
