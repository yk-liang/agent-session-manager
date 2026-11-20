import assert from 'node:assert/strict';

// 三击只选中所在段落：WebKit 原生会把段落分隔符（下一个单元格的开头）一起选中。
export async function assertTripleClickSelects(locator, expected = null) {
  await locator.click({ clickCount: 3 });
  const result = await locator.evaluate(element => {
    const selection = getSelection();
    const range = selection.rangeCount ? selection.getRangeAt(0) : null;
    return {
      text: selection.toString(),
      contained: !!range && element.contains(range.startContainer) && element.contains(range.endContainer),
      content: element.textContent,
    };
  });
  await locator.page().evaluate(() => getSelection().removeAllRanges());
  assert.equal(result.text, expected ?? result.content, 'triple click selects the paragraph without the trailing break');
  assert.ok(result.contained, 'triple-click selection stays inside the clicked element');
}
