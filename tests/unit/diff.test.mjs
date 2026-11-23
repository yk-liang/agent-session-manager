import test from "node:test";
import assert from "node:assert/strict";
import { foldUnchanged, hunkRows, lineDiff } from "../../src/diff.ts";

for (const [label, before, after] of [
  ["add file", null, "a\nb\n"], ["remove file", "a\nb", null],
  ["empty versus absent", null, ""], ["trailing newline", "a", "a\n"],
  ["repeated lines", "a\nb\na\nb", "b\na\nb\na"],
  ["unicode and blank lines", "模型\n\n旧值", "模型\n\n新值\n"],
  ["identical", "a\n", "a\n"], ["absent", null, null],
  ["bounded large diff", Array.from({ length: 1100 }, (_, i) => `old${i}`).join("\n"), Array.from({ length: 1100 }, (_, i) => `new${i}`).join("\n")],
]) test(label, () => {
  const rows = lineDiff(before, after);
  assert.equal(rows.filter((r) => r.kind !== "add").map((r) => r.text).join("\n"), before ?? "");
  assert.equal(rows.filter((r) => r.kind !== "remove").map((r) => r.text).join("\n"), after ?? "");
  assert.deepEqual(rows.filter((r) => r.before !== null).map((r) => r.before), Array.from({ length: before === null ? 0 : before.split("\n").length }, (_, i) => i + 1));
  assert.deepEqual(rows.filter((r) => r.after !== null).map((r) => r.after), Array.from({ length: after === null ? 0 : after.split("\n").length }, (_, i) => i + 1));
});

test("hunks keep line numbers and fold distant context", () => {
  const rows = hunkRows([
    { old_start: 3, new_start: 3, lines: [" a", " b", " c", "-d", "+D", "+E", " f"] },
    { old_start: 20, new_start: 21, lines: [" x", "+y"] },
  ]);
  assert.deepEqual(rows.map((row) => row.kind === "skip" ? `skip ${row.count}` : `${row.before}/${row.after}`),
    ["3/3", "4/4", "5/5", "6/null", "null/6", "null/7", "7/8", "skip 12", "20/21", "null/22"]);
  assert.deepEqual(foldUnchanged(rows, 1).map((row) => row.kind === "skip" ? `skip ${row.count}` : row.text),
    ["skip 2", "c", "d", "D", "E", "f", "skip 12", "x", "y"]);
  assert.deepEqual(foldUnchanged(hunkRows([{ old_start: null, new_start: null, lines: [" a", " b", "+c"] }, { old_start: null, new_start: null, lines: [" z"] }]), 0)
    .map((row) => row.kind === "skip" ? `skip ${row.count}` : row.text), ["skip 2", "c", "skip null"]);
});
