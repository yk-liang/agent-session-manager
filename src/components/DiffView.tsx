import { foldUnchanged, hunkRows, type DiffRow } from "../diff";
import type { DiffHunk } from "../types";
import { ExpandableFrame } from "./ToolContent";

export default function DiffView({ files }: { files: { path?: string; hunks: DiffHunk[] }[] }) {
  const groups = files.map(file => {
    const rows = hunkRows(file.hunks);
    return { path: file.path, rows, folded: foldUnchanged(rows, 2) };
  });
  const lines = groups.reduce((sum, group) => sum + group.rows.length, 0);
  const truncated = groups.some(({ rows, folded }) => folded.length !== rows.length
    || folded.some((row, index) => row.kind !== rows[index].kind));
  return <ExpandableFrame lines={lines} truncated={truncated} label="改动">{expanded => groups.map((group, index) =>
    <div key={index} className="tool-diff-file">
      {group.path && <div className="search-file-name" title={group.path}>{group.path}</div>}
      <DiffRows rows={expanded ? group.rows : group.folded} />
    </div>
  )}</ExpandableFrame>;
}

function DiffRows({ rows }: { rows: DiffRow[] }) {
  return <div className="tool-diff">{rows.map((row, index) => row.kind === "skip"
    ? <div key={index} className="tool-diff-skip">{row.count === null ? "⋯" : `⋯ ${row.count} 行未改动`}</div>
    : <div key={index} className={`tool-diff-row ${row.kind}`}>
      <span className="tool-diff-number">{row.after ?? row.before ?? ""}</span>
      <span className="tool-diff-sign">{row.kind === "add" ? "+" : row.kind === "remove" ? "−" : ""}</span>
      <span className="tool-diff-text">{row.text || " "}</span>
    </div>)}</div>;
}
