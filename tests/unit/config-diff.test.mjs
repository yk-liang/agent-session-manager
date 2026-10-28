import test from "node:test";
import assert from "node:assert/strict";
import { configDiff } from "../../src/configDiff.ts";

for (const [label, before, after] of [
  ["add file", null, "a\nb\n"], ["remove file", "a\nb", null],
  ["empty versus absent", null, ""], ["trailing newline", "a", "a\n"],
  ["repeated lines", "a\nb\na\nb", "b\na\nb\na"],
  ["unicode and blank lines", "模型\n\n旧值", "模型\n\n新值\n"],
  ["identical", "a\n", "a\n"], ["absent", null, null],
  ["bounded large diff", Array.from({ length: 1100 }, (_, i) => `old${i}`).join("\n"), Array.from({ length: 1100 }, (_, i) => `new${i}`).join("\n")],
]) test(label, () => {
  const rows = configDiff(before, after);
  assert.equal(rows.filter((r) => r.kind !== "add").map((r) => r.text).join("\n"), before ?? "");
  assert.equal(rows.filter((r) => r.kind !== "remove").map((r) => r.text).join("\n"), after ?? "");
  assert.deepEqual(rows.filter((r) => r.before !== null).map((r) => r.before), Array.from({ length: before === null ? 0 : before.split("\n").length }, (_, i) => i + 1));
  assert.deepEqual(rows.filter((r) => r.after !== null).map((r) => r.after), Array.from({ length: after === null ? 0 : after.split("\n").length }, (_, i) => i + 1));
});
