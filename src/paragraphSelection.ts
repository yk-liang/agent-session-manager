// macOS 上 Tauri 使用 WKWebView。WebKit 三击会连同段落分隔符一起选中，选区终点落到下一个段落，
// 在表格和描述列表里就是相邻单元格：复制时多出换行，相邻单元格的空白也被绘制成高亮。
// 这里只选中段落本身；段落边界仍由引擎计算，行为与 Chromium 的三击一致。
function selectParagraph(event: MouseEvent) {
  if (event.detail < 3 || event.button !== 0 || event.defaultPrevented) return;
  if (event.shiftKey || event.altKey || event.metaKey || event.ctrlKey) return;
  const target = event.target;
  if (!(target instanceof Element) || target.closest("input, textarea, select, [contenteditable]")) return;
  // WebKit 只按带前缀的属性决定能否选中。
  const mode = getComputedStyle(target).webkitUserSelect;
  if (mode === "none" || mode === "all") return;
  const caret = document.caretRangeFromPoint(event.clientX, event.clientY);
  // 落点不在被点击元素内（例如点在按钮图标上）时交给默认行为。
  const selection = getSelection();
  if (!caret || !selection || !target.contains(caret.startContainer)) return;
  selection.collapse(caret.startContainer, caret.startOffset);
  selection.modify("move", "backward", "paragraphboundary");
  selection.modify("extend", "forward", "paragraphboundary");
  // 空段落仍交给默认三击。
  if (!selection.isCollapsed) event.preventDefault();
}

export function installParagraphSelection() {
  document.addEventListener("mousedown", selectParagraph, true);
}
