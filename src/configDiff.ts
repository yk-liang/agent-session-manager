export interface DiffLine {
  kind: "same" | "add" | "remove";
  text: string;
  before: number | null;
  after: number | null;
}

/** 保留空行与文件末尾换行差异；大文件降级为完整替换，避免 LCS 阻塞窗口。 */
export function configDiff(
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
