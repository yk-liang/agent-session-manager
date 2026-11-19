import assert from 'node:assert/strict';

export async function assertTooltipValues(tooltip) {
  await tooltip.waitFor();
  const { edge, values } = await tooltip.evaluate(element => {
    const style = getComputedStyle(element);
    const edge = element.getBoundingClientRect().right
      - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight);
    const values = [...element.querySelectorAll('dd')].map(value => {
      const range = document.createRange();
      range.selectNodeContents(value);
      return { text: value.textContent, right: range.getBoundingClientRect().right };
    });
    return { edge, values };
  });
  assert.ok(values.length > 0, 'expected tooltip values');
  for (const { text, right } of values) {
    assert.ok(Math.abs(right - edge) < 1, `tooltip value “${text}” ends at ${right}, expected ${edge}`);
  }
}
