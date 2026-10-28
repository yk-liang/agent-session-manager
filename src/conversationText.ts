import type { SessionEvent } from "./types";

export function eventTextParts(event: SessionEvent): string[] {
  if (
    (event.type !== "user" && event.type !== "assistant") ||
    (event.type === "user" && event.isMeta)
  )
    return [];
  const content = event.message?.content;
  const parts =
    typeof content === "string"
      ? [content]
      : Array.isArray(content)
        ? content.flatMap((part) =>
            part && part.type === "text" && typeof part.text === "string"
              ? [part.text]
              : [],
          )
        : [];
  return parts.map((text) => text.trim()).filter(Boolean);
}

function foldCase(text: string): string {
  let folded = "";
  for (const char of text) folded += char.toLowerCase();
  return folded;
}

/** 返回原文的 UTF-16 范围；大小写折叠变长及 emoji 都不会切坏高亮字符。 */
export function matchRanges(
  text: string,
  query: string,
  limit = 200,
): [number, number][] {
  const needle = foldCase(query.trim());
  if (!needle) return [];
  const folded = foldCase(text);
  const matches: [number, number][] = [];
  let cursor = 0;
  while (matches.length < limit) {
    const start = folded.indexOf(needle, cursor);
    if (start < 0) break;
    matches.push([start, start + needle.length]);
    cursor = start + needle.length;
  }
  const ranges: [number, number][] = [];
  let foldedOffset = 0,
    sourceOffset = 0,
    current = 0,
    sourceStart = -1;
  for (const char of text) {
    if (current === matches.length) break;
    const next = foldedOffset + char.toLowerCase().length;
    while (current < matches.length && matches[current][0] < next) {
      if (sourceStart < 0) sourceStart = sourceOffset;
      if (matches[current][1] > next) break;
      const end = sourceOffset + char.length;
      const previous = ranges[ranges.length - 1];
      if (previous && previous[1] >= sourceStart) previous[1] = end;
      else ranges.push([sourceStart, end]);
      sourceStart = -1;
      current++;
    }
    foldedOffset = next;
    sourceOffset += char.length;
  }
  return ranges;
}

export function eventMatches(event: SessionEvent, query: string): boolean {
  return eventTextParts(event).some(
    (text) => matchRanges(text, query, 1).length > 0,
  );
}
