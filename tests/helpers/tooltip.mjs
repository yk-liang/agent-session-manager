import assert from 'node:assert/strict';

const INK = 24;

// 检查实际绘制的像素，避免文字越过排版边界时仍通过几何断言。
async function paintedRight(page, clip, bands, background) {
  const png = await page.screenshot({ clip, scale: 'device', animations: 'disabled' });
  return page.evaluate(async ({ data, clip, bands, background: [r, g, b], INK }) => {
    const image = new Image();
    image.src = `data:image/png;base64,${data}`;
    await image.decode();
    const canvas = Object.assign(document.createElement('canvas'), { width: image.naturalWidth, height: image.naturalHeight });
    const context = canvas.getContext('2d', { willReadFrequently: true });
    context.drawImage(image, 0, 0);
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
    const scale = canvas.width / clip.width;
    return bands.map(({ top, bottom }) => {
      const y0 = Math.max(0, Math.floor((top - clip.y) * scale)), y1 = Math.min(canvas.height, Math.ceil((bottom - clip.y) * scale));
      for (let x = canvas.width - 1; x >= 0; x--) {
        for (let y = y0; y < y1; y++) {
          const i = (y * canvas.width + x) * 4;
          if (Math.abs(pixels[i] - r) + Math.abs(pixels[i + 1] - g) + Math.abs(pixels[i + 2] - b) > INK) return clip.x + (x + 1) / scale;
        }
      }
      return null;
    });
  }, { data: png.toString('base64'), clip, bands, background, INK });
}

export async function assertTooltipValues(tooltip, expected) {
  await tooltip.waitFor();
  const { bounds, viewport, edge, padding, background, values, rule } = await tooltip.evaluate(element => {
    const style = getComputedStyle(element);
    const { left, right, top, bottom } = element.getBoundingClientRect();
    const padding = parseFloat(style.paddingRight);
    const values = [...element.querySelectorAll('dd')].map(value => {
      const range = document.createRange();
      range.selectNodeContents(value);
      const lines = new Set([...range.getClientRects()].map(rect => Math.round(rect.top))).size;
      const text = range.getBoundingClientRect();
      return { text: value.textContent, lines, overflow: value.scrollWidth - value.clientWidth, right: text.right, top: text.top, bottom: text.bottom };
    });
    const total = element.querySelector('.usage-tooltip-total');
    const ruleWidth = total ? parseFloat(getComputedStyle(total).borderTopWidth) : 0;
    const totalBox = total?.getBoundingClientRect();
    return {
      bounds: { left, right, top, bottom },
      viewport: { width: innerWidth, height: innerHeight },
      edge: right - parseFloat(style.borderRightWidth) - padding,
      padding,
      background: style.backgroundColor.match(/[\d.]+/g).slice(0, 3).map(Number),
      values,
      rule: ruleWidth > 0 ? { right: totalBox.right, top: totalBox.top, bottom: totalBox.top + ruleWidth } : null,
    };
  });
  assert.ok(bounds.left >= 7 && bounds.right <= viewport.width - 7, 'tooltip fits horizontally');
  assert.ok(bounds.top >= 7 && bounds.bottom <= viewport.height - 7, 'tooltip fits vertically');
  assert.ok(values.length > 0, 'expected tooltip values');
  if (expected) assert.deepEqual(values.map(value => value.text), expected);
  for (const { text, right, lines, overflow } of values) {
    assert.equal(lines, 1, `tooltip value “${text}” stays on one line`);
    assert.ok(overflow <= 0, `tooltip value “${text}” overflows its box by ${overflow}px`);
    assert.ok(Math.abs(right - edge) < 1, `tooltip value “${text}” is laid out to ${right}, expected ${edge}`);
  }
  if (rule) assert.ok(Math.abs(rule.right - edge) < 1, `total divider is laid out to ${rule.right}, expected ${edge}`);
  // 截图带从内容区右缘左侧 12px 起，止于右内边距中段，提示框边框不在其中。
  const x = Math.floor(edge - 12);
  const clip = { x, y: Math.floor(bounds.top), width: Math.floor(edge + padding / 2) - x, height: Math.ceil(bounds.bottom) - Math.floor(bounds.top) };
  const painted = await paintedRight(tooltip.page(), clip, rule ? [...values, rule] : values, background);
  for (const [i, { text }] of values.entries()) {
    assert.ok(painted[i] != null && painted[i] <= edge + 1 && painted[i] >= edge - 3, `tooltip value “${text}” is painted to ${painted[i]}, expected ${edge}`);
  }
  if (rule) assert.ok(painted.at(-1) != null && Math.abs(painted.at(-1) - edge) <= 1, `total divider is painted to ${painted.at(-1)}, expected ${edge}`);
}
