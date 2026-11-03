import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { createServer } from 'vite';
import { webkit } from 'playwright';
import { availablePort } from '../tests/helpers/server.mjs';
import { chooseAgent } from '../tests/helpers/agent-switcher.mjs';
import { installScreenshotFixture, screenshotFixture, screenshotTime } from './readme-fixture.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const output = path.join(root, 'docs/images');

// WebKit may attach EXIF. Remove metadata without changing the captured pixels.
function withoutMetadata(png) {
  const chunks = [png.subarray(0, 8)];
  for (let offset = 8; offset < png.length;) {
    const end = offset + png.readUInt32BE(offset) + 12;
    const type = png.subarray(offset + 4, offset + 8).toString();
    if (!['eXIf', 'tEXt', 'iTXt', 'zTXt'].includes(type)) chunks.push(png.subarray(offset, end));
    offset = end;
  }
  return Buffer.concat(chunks);
}

let server, browser;
try {
  // Serve the real frontend, but never start Tauri or access any user's Agent data.
  server = await createServer({ root, server: { host: '127.0.0.1', port: await availablePort(), strictPort: true, open: false } });
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await webkit.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, locale: 'zh-CN', timezoneId: 'America/Los_Angeles', reducedMotion: 'reduce' });
  const errors = [];
  // Only local frontend assets may load. An unexpected request fails the capture.
  await context.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin === origin) await route.continue();
    else { errors.push(`Unexpected external request: ${url.origin}`); await route.abort(); }
  });
  await context.addInitScript(installScreenshotFixture, screenshotFixture);
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date(screenshotTime));
  await page.goto(origin);
  await mkdir(output, { recursive: true });

  async function capture(name, target = page) {
    await page.mouse.move(0, 0);
    await page.evaluate(() => document.fonts.ready);
    const text = await page.locator('body').innerText();
    assert.doesNotMatch(text, /\/Users\/(?!demo\b)[^/\s]+|sk-[a-zA-Z0-9]{10,}|[\w.+-]+@[\w.-]+\.[a-z]{2,}/, 'Unexpected private data in screenshot');
    assert.deepEqual(errors, []);
    const png = await target.screenshot({ animations: 'disabled' });
    await writeFile(path.join(output, `${name}.png`), withoutMetadata(png));
    console.log(`Saved docs/images/${name}.png (synthetic data)`);
  }

  await chooseAgent(page, 'codex');
  await page.locator('.session-card', { hasText: '给笔记应用添加全文搜索' }).click();
  await page.locator('.markdown-body h2').waitFor();
  await capture('sessions');

  await page.getByRole('button', { name: '用量', exact: true }).click();
  await chooseAgent(page, 'all');
  await page.getByRole('button', { name: '近 7 天', exact: true }).click();
  await page.getByText('第 1–50 条，共 180 条', { exact: true }).waitFor();
  await page.waitForFunction(() => document.querySelector('.usage-stat > span')?.textContent === '总 Token');
  // A wider window lets the real request table show every column without altering its CSS.
  await page.setViewportSize({ width: 1664, height: 1040 });
  await page.waitForFunction(() => document.querySelector('.usage-chart').clientWidth > 1400);
  await capture('usage');
  await capture('models', page.locator('.usage-models'));

  await chooseAgent(page, 'codex');
  await page.getByRole('button', { name: '配置', exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.locator('[data-file-key="config.toml"] .collapsible-head').click();
  await page.locator('.config-code').waitFor();
  await page.locator('.config-history h2 button').click();
  await page.locator('.backup-row').first().waitFor();
  await capture('configuration');
  assert.deepEqual(errors, []);
} finally {
  await browser?.close();
  await server?.close();
}
