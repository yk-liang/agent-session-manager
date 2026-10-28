import { useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import UiIcon from "./UiIcon";
import { configDiff } from "../configDiff";
import type { ConfigFileChange } from "../types";

const actionLabels = {
  create: "新增",
  replace: "覆盖",
  remove: "移除",
  unchanged: "不变",
};

export default function ConfigFileDiff({ file }: { file: ConfigFileChange }) {
  const rows = useMemo(
    () => configDiff(file.before, file.after),
    [file.before, file.after],
  );
  const [showAll, setShowAll] = useState(false);
  const visible = useMemo(() => {
    if (showAll || rows.length < 24)
      return rows.map((row, index) => ({ row, index }));
    const indexes = new Set<number>();
    rows.forEach((row, index) => {
      if (row.kind !== "same")
        for (
          let i = Math.max(0, index - 3);
          i <= Math.min(rows.length - 1, index + 3);
          i++
        )
          indexes.add(i);
    });
    return [...indexes]
      .sort((a, b) => a - b)
      .map((index) => ({ row: rows[index], index }));
  }, [rows, showAll]);

  return (
    <details className="config-diff-file" open={file.action !== "unchanged"}>
      <summary>
        <UiIcon icon={ChevronRight} className="details-caret" />
        <span className={`change-tag ${file.action}`}>
          {actionLabels[file.action]}
        </span>
        <strong>{file.label}</strong>
        <span className="diff-path">{file.path}</span>
      </summary>
      {file.action === "remove" && (
        <p className="diff-removal">此文件将移入回收站。</p>
      )}
      {file.action === "unchanged" ? (
        <p className="hint diff-empty">文件内容与目标一致。</p>
      ) : (
        <>
          <div
            className="config-diff-code"
            aria-label={`${file.label} 内容差异`}
          >
            <div className="diff-legend">
              <span>原行</span>
              <span>新行</span>
              <span>− 移除 / + 新增</span>
            </div>
            {visible.map(({ row, index }, position) => (
              <div key={index}>
                {index > (position ? visible[position - 1].index + 1 : 0) && (
                  <div className="diff-gap">
                    … 省略{" "}
                    {index - (position ? visible[position - 1].index + 1 : 0)}{" "}
                    行未变化内容
                  </div>
                )}
                <div className={`diff-line ${row.kind}`}>
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
              </div>
            ))}
            {visible.length > 0 &&
              visible[visible.length - 1].index < rows.length - 1 && (
                <div className="diff-gap">… 其余内容未变化</div>
              )}
          </div>
          {rows.length >= 24 && (
            <button
              className="text-button diff-expand"
              onClick={() => setShowAll(!showAll)}
            >
              {showAll ? "仅看变更附近" : "展开完整差异"}
            </button>
          )}
        </>
      )}
    </details>
  );
}
