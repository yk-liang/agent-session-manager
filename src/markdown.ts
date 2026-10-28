import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import { matchRanges } from "./conversationText.ts";

type MarkdownNode = { type: string; value?: string; position?: { start: { offset?: number } }; children?: MarkdownNode[] };
type HtmlNode = { type: string; value?: string; tagName?: string; properties?: Record<string, unknown>; children?: HtmlNode[] };
const parser = unified().use(remarkParse).use(remarkGfm);
const formatting = new Set(["heading", "list", "blockquote", "code", "inlineCode", "emphasis", "strong", "delete", "link", "image", "table", "thematicBreak", "break", "footnoteReference"]);

/** 用真实语法树识别 Markdown；纯段落与日志不改变空白、换行或标签文字。只处理当前可见片段。 */
export function canRenderMarkdown(text: string, query = ""): boolean {
  const start = text.trimStart();
  // 原始 JSON/JSON 片段中的星号、下划线属于数据，不按行内 Markdown 处理。
  if (start.startsWith("{") || /^\[\s*["{\[]/.test(start)) return false;
  if (!/[#*_`~\[\]<>|\\\-+=.!:]|(?:^|\n) {4}/.test(text)) return false;
  const root = parser.parse(text) as MarkdownNode;
  let formatted = false, visibleMatch = !query;
  const visit = (node: MarkdownNode) => {
    const offset = node.position?.start.offset;
    // 普通消息中的裸 URL/邮箱不单独触发格式转换；显式链接和已有 Markdown 中的链接仍正常渲染。
    const explicitLink = node.type !== "link" || (offset !== undefined && (text[offset] === "[" || text[offset] === "<"));
    if (formatting.has(node.type) && explicitLink) formatted = true;
    if (node.type !== "html" && typeof node.value === "string" && query && matchRanges(node.value, query, 1).length) visibleMatch = true;
    node.children?.forEach(visit);
  };
  visit(root);
  // 搜索命中标记或链接地址、但渲染后的正文没有该字串时，保留此片段原文以便定位。
  return formatted && visibleMatch;
}

/** 在已解析的文本节点中插入高亮，不把搜索词插进 Markdown 源码或 HTML 字符串。 */
export function highlightMarkdown(options: { query: string }) {
  return (root: HtmlNode) => {
    if (!options.query) return;
    let remaining = 200;
    const visit = (node: HtmlNode) => {
      if (!node.children || node.tagName === "mark") return;
      const next: HtmlNode[] = [];
      for (const child of node.children) {
        if (child.type !== "text" || !child.value || !remaining) { visit(child); next.push(child); continue; }
        const ranges = matchRanges(child.value, options.query, remaining);
        remaining -= ranges.length;
        let cursor = 0;
        for (const [start, end] of ranges) {
          if (start > cursor) next.push({ type: "text", value: child.value.slice(cursor, start) });
          next.push({ type: "element", tagName: "mark", properties: { className: ["search-highlight"] }, children: [{ type: "text", value: child.value.slice(start, end) }] });
          cursor = end;
        }
        if (cursor < child.value.length) next.push({ type: "text", value: child.value.slice(cursor) });
      }
      node.children = next;
    };
    visit(root);
  };
}

export function externalMarkdownLink(href: string): boolean {
  return /^(https?:\/\/|mailto:)/i.test(href);
}
