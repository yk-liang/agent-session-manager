import type { DiffHunk } from "./types";

export interface DiffLine {
  kind: "same" | "add" | "remove";
  text: string;
  before: number | null;
  after: number | null;
}
/** 折叠掉的未改动行；行数未知时为空。 */
type DiffSkip = { kind: "skip"; count: number | null };
export type DiffRow = DiffLine | DiffSkip;

/** 保留空行与文件末尾换行差异；大文件降级为完整替换，避免 LCS 阻塞窗口。 */
export function lineDiff(
  before: string | null,
  after: string | null,
): DiffLine[] {
  const a = before === null ? [] : before.split("\n");
  const b = after === null ? [] : after.split("\n");
  let prefix = 0;
  while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix])
    prefix++;
  let suffix = 0;
  while (
    suffix < a.length - prefix &&
    suffix < b.length - prefix &&
    a[a.length - 1 - suffix] === b[b.length - 1 - suffix]
  )
    suffix++;
  const rows: DiffLine[] = [];
  for (let i = 0; i < prefix; i++)
    rows.push({ kind: "same", text: a[i], before: i + 1, after: i + 1 });
  const n = a.length - prefix - suffix;
  const m = b.length - prefix - suffix;
  if (n * m > 1_000_000) {
    for (let i = 0; i < n; i++)
      rows.push({
        kind: "remove",
        text: a[prefix + i],
        before: prefix + i + 1,
        after: null,
      });
    for (let j = 0; j < m; j++)
      rows.push({
        kind: "add",
        text: b[prefix + j],
        before: null,
        after: prefix + j + 1,
      });
  } else {
    const width = m + 1;
    const lcs = new Uint32Array((n + 1) * width);
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        lcs[i * width + j] =
          a[prefix + i] === b[prefix + j]
            ? lcs[(i + 1) * width + j + 1] + 1
            : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
      }
    }
    let i = 0,
      j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && a[prefix + i] === b[prefix + j]) {
        rows.push({
          kind: "same",
          text: a[prefix + i],
          before: prefix + i + 1,
          after: prefix + j + 1,
        });
        i++;
        j++;
      } else if (
        i < n &&
        (j === m || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
      ) {
        rows.push({
          kind: "remove",
          text: a[prefix + i],
          before: prefix + i + 1,
          after: null,
        });
        i++;
      } else {
        rows.push({
          kind: "add",
          text: b[prefix + j],
          before: null,
          after: prefix + j + 1,
        });
        j++;
      }
    }
  }
  for (let i = suffix; i > 0; i--)
    rows.push({
      kind: "same",
      text: a[a.length - i],
      before: a.length - i + 1,
      after: b.length - i + 1,
    });
  return rows;
}

const next = (line: number | null) => line === null ? null : line + 1;

/** 工具补丁的 hunk 转为差异行，相邻 hunk 之间标出跳过的行数。 */
export function hunkRows(hunks: DiffHunk[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let lastBefore: number | null = null;
  hunks.forEach((hunk, index) => {
    let before = hunk.old_start, after = hunk.new_start;
    if (index > 0) rows.push({ kind: "skip", count: before !== null && lastBefore !== null ? before - lastBefore - 1 : null });
    for (const line of hunk.lines) {
      const text = line.slice(1);
      if (line[0] === "+") {
        rows.push({ kind: "add", text, before: null, after });
        after = next(after);
      } else {
        rows.push({ kind: line[0] === "-" ? "remove" : "same", text, before, after: line[0] === "-" ? null : after });
        lastBefore = before;
        before = next(before);
        if (line[0] !== "-") after = next(after);
      }
    }
  });
  return rows;
}

/** 只保留改动前后 `context` 行内的未改动行，其余合并为跳过标记。 */
export function foldUnchanged(rows: DiffRow[], context: number): DiffRow[] {
  const near = new Uint8Array(rows.length);
  rows.forEach((row, index) => {
    if (row.kind === "add" || row.kind === "remove") near.fill(1, Math.max(0, index - context), index + context + 1);
  });
  const folded: DiffRow[] = [];
  rows.forEach((row, index) => {
    const skip: DiffSkip | undefined = row.kind === "skip" ? row : row.kind === "same" && !near[index] ? { kind: "skip", count: 1 } : undefined;
    const last = folded[folded.length - 1];
    if (!skip) folded.push(row);
    else if (last?.kind === "skip") last.count = last.count === null || skip.count === null ? null : last.count + skip.count;
    else folded.push({ ...skip });
  });
  return folded;
}
