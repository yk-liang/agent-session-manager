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
const output = process.argv[2] ? path.resolve(process.argv[2]) : path.join(root, 'docs/images');
const deviceScaleFactor = 2;

const windowPreview = `
  <style>
    body { margin: 0; font: 500 13px -apple-system, BlinkMacSystemFont, sans-serif; }
    .capture { width: max-content; padding: 24px 30px 36px; }
    .window {
      overflow: hidden;
      border: 1px solid #51545b;
      border-radius: 12px;
      background: #181a20;
      box-shadow: 0 9px 22px rgb(0 0 0 / 25%), 0 2px 6px rgb(0 0 0 / 18%);
    }
    .title-bar {
      position: relative;
      display: grid;
      place-items: center;
      height: 36px;
      border-bottom: 1px solid #15171b;
      background: linear-gradient(#303238, #27292f);
      color: #e0e1e5;
    }
    .window-controls { position: absolute; left: 13px; display: flex; gap: 8px; }
    .window-controls span { width: 12px; height: 12px; border-radius: 50%; background: var(--color); }
    img { display: block; }
  </style>
  <main class="capture">
    <div class="window">
      <div class="title-bar">
        <div class="window-controls" aria-hidden="true">
          <span style="--color: #ff5f57"></span>
          <span style="--color: #febc2e"></span>
          <span style="--color: #28c840"></span>
        </div>
        Agent Session Manager
      </div>
      <img alt="">
    </div>
  </main>
`;

async function frameScreenshot(preview, png) {
  const image = preview.locator('img');
  await image.evaluate(async (element, { source, scale }) => {
    element.src = source;
    await element.decode();
    element.style.width = `${element.naturalWidth / scale}px`;
  }, { source: `data:image/png;base64,${png.toString('base64')}`, scale: deviceScaleFactor });
  return preview.locator('.capture').screenshot({ omitBackground: true });
}

// Strip incidental metadata added by WebKit.
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
  server = await createServer({ root, server: { host: '127.0.0.1', port: await availablePort(), strictPort: true, open: false } });
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await webkit.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 }, deviceScaleFactor,
    locale: 'zh-CN', timezoneId: 'America/Los_Angeles', colorScheme: 'dark', reducedMotion: 'reduce',
  });
  const errors = [];
  // Fail if the screenshot would require an external asset.
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
  const preview = await browser.newPage({ deviceScaleFactor });
  await preview.setContent(windowPreview);
  await mkdir(output, { recursive: true });

  async function capture(name) {
    await page.mouse.move(0, 0);
    await page.evaluate(() => document.fonts.ready);
    const text = await page.locator('body').innerText();
    assert.doesNotMatch(text, /\/Users\/(?!demo\b)[^/\s]+|sk-[a-zA-Z0-9]{10,}|[\w.+-]+@[\w.-]+\.[a-z]{2,}/, 'Unexpected private data in screenshot');
    assert.deepEqual(errors, []);
    const screenshot = await page.screenshot({ animations: 'disabled' });
    const png = await frameScreenshot(preview, screenshot);
    const destination = path.join(output, `${name}.png`);
    await writeFile(destination, withoutMetadata(png));
    console.log(`Saved ${destination} (synthetic data)`);
  }

  await chooseAgent(page, 'codex');
  await page.locator('.session-card', { hasText: '给笔记应用添加全文搜索' }).click();
  await page.locator('.markdown-body h2').waitFor();
  await capture('sessions');

  await page.getByRole('button', { name: '用量', exact: true }).click();
  await chooseAgent(page, 'all');
  await page.getByRole('button', { name: '近 7 天', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.usage-stat[data-metric="tokens"] > span')?.textContent === 'Token 用量');
  await page.locator('.usage-models tbody tr').first().waitFor();
  await page.setViewportSize({ width: 1664, height: 1040 });
  await page.waitForFunction(() => document.querySelector('.usage-chart').clientWidth > 1400);
  await capture('usage');

  await chooseAgent(page, 'codex');
  await page.getByRole('button', { name: '配置', exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.locator('[data-file-key="config.toml"] .config-file-toggle').click();
  await page.locator('.config-code').waitFor();
  await page.locator('.config-history h2 button').click();
  await page.locator('.backup-row').first().waitFor();
  await capture('configuration');
  assert.deepEqual(errors, []);
} finally {
  await browser?.close();
  await server?.close();
}
