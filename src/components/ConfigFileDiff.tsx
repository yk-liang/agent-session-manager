import { useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import UiIcon from "./UiIcon";
import { foldUnchanged, lineDiff } from "../diff";
import type { ConfigFileChange } from "../types";

const actionTones = { create: "success", replace: "warning", remove: "error", unchanged: undefined };

const actionLabels = {
  create: "创建",
  replace: "覆盖",
  remove: "删除",
  unchanged: "不变",
};

export default function ConfigFileDiff({ file }: { file: ConfigFileChange }) {
  const rows = useMemo(
    () => lineDiff(file.before, file.after),
    [file.before, file.after],
  );
  const [showAll, setShowAll] = useState(false);
  const visible = useMemo(
    () => (showAll || rows.length < 24 ? rows : foldUnchanged(rows, 3)),
    [rows, showAll],
  );

  return (
    <details className="config-diff-file" open={file.action !== "unchanged"}>
      <summary>
        <UiIcon icon={ChevronRight} className="details-caret" />
        <span className="status-tag" data-tone={actionTones[file.action]}>
          {actionLabels[file.action]}
        </span>
        <strong>{file.label}</strong>
        <span className="diff-path">{file.path}</span>
      </summary>
      {file.action === "remove" && (
        <p className="diff-removal">此文件将移入废纸篓。</p>
      )}
      {file.action === "unchanged" ? (
        <p className="hint diff-empty">内容一致。</p>
      ) : (
        <>
          <div
            className="config-diff-code"
            aria-label={`${file.label} 内容差异`}
          >
            <div className="diff-legend">
              <span>原行</span>
              <span>新行</span>
              <span>− 删除 / + 新增</span>
            </div>
            {visible.map((row, index) =>
              row.kind === "skip" ? (
                <div key={index} className="diff-gap">
                  … 省略 {row.count} 行未改动内容
                </div>
              ) : (
                <div key={index} className={`diff-line ${row.kind}`}>
                  <span>{row.before ?? ""}</span>
                  <span>{row.after ?? ""}</span>
                  <code>
                    {row.kind === "add"
                      ? "+"
                      : row.kind === "remove"
                        ? "−"
                        : " "}{" "}
                    {row.text || " "}
                  </code>
                </div>
              ),
            )}
          </div>
          {rows.length >= 24 && (
            <button
              type="button"
              className="btn-ghost btn-sm diff-expand"
              onClick={() => setShowAll(!showAll)}
            >
              {showAll ? "收起未改动的行" : "展开完整差异"}
            </button>
          )}
        </>
      )}
    </details>
  );
}
