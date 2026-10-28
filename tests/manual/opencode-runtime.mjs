// Runs the installed official v2 CLI against a local model endpoint. All config,
// databases, credentials and project files live in a disposable directory.
import { engine, baseURL, root as repo } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import { createServer } from 'node:http';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { once } from 'node:events';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';

const exec = promisify(execFile);
const root = mkdtempSync(path.join(tmpdir(), 'asm-opencode-runtime-'));
const project = path.join(root, 'project');
const bin = process.env.OPENCODE_BIN || 'opencode';
for (const dir of ['project', 'config/opencode', 'data', 'cache', 'state', 'home']) {
  mkdirSync(path.join(root, dir), { recursive: true });
}
writeFileSync(path.join(project, 'example.txt'), 'Actual OpenCode v2 integration fixture.\n');
const env = { ...process.env, ASM_LIVE_KEY: 'local-test-only', OPENCODE_SERVER_PASSWORD: 'local-test-only',
  OPENCODE_DISABLE_MODELS_FETCH: 'true', OPENCODE_TEST_HOME: path.join(root, 'home'),
  XDG_CONFIG_HOME: path.join(root, 'config'), XDG_DATA_HOME: path.join(root, 'data'),
  XDG_CACHE_HOME: path.join(root, 'cache'), XDG_STATE_HOME: path.join(root, 'state') };
let requests = 0;
const model = createServer(async (req, res) => {
  try {
    let raw = '';
    for await (const chunk of req) raw += chunk;
    const body = JSON.parse(raw);
    requests++;
    const read = body.tools?.find(t => t.function?.name === 'read');
    const tool = read && !body.messages?.some(m => m.role === 'tool');
    const props = read?.function.parameters?.properties ?? {};
    const key = 'filePath' in props ? 'filePath' : 'path' in props ? 'path' : 'file_path';
    const delta = tool ? { role: 'assistant', tool_calls: [{ index: 0, id: 'call_asm_live', type: 'function',
      function: { name: 'read', arguments: JSON.stringify({ [key]: path.join(project, 'example.txt') }) } }] }
      : { role: 'assistant', content: 'ASM OpenCode v2 live answer' };
    assert.equal(body.stream, true);
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    for (const [part, finish] of [[delta, null], [{}, tool ? 'tool_calls' : 'stop']]) {
      res.write(`data: ${JSON.stringify({ id: `chatcmpl-${requests}`, object: 'chat.completion.chunk',
        created: Math.floor(Date.now() / 1000), model: 'live', choices: [{ index: 0, delta: part, finish_reason: finish }],
        ...(finish ? { usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110,
          prompt_tokens_details: { cached_tokens: 5 }, completion_tokens_details: { reasoning_tokens: 2 } } } : {}) })}\n\n`);
    }
    res.end('data: [DONE]\n\n');
  } catch (error) { res.writeHead(500); res.end(String(error)); }
});
let native;
let browser;
async function run(command, args, options = {}) {
  const pending = exec(command, args, { cwd: project, env, timeout: 90000, maxBuffer: 4 * 1024 * 1024, ...options });
  // `opencode run` also consumes piped stdin. EOF is required even with a prompt argument.
  pending.child.stdin?.end();
  return pending;
}
async function stopNative() {
  if (!native || native.exitCode !== null || native.signalCode !== null) return;
  const exited = once(native, 'exit');
  native.kill('SIGTERM');
  const timer = setTimeout(() => native.kill('SIGKILL'), 10000);
  try { await exited; } finally { clearTimeout(timer); }
}
try {
  const version = (await run(bin, ['--version'])).stdout.trim();
  assert.match(version, /v2\./);
  await new Promise(resolve => model.listen(0, '127.0.0.1', resolve));
  writeFileSync(path.join(root, 'config/opencode/opencode.json'), JSON.stringify({
    model: 'asm-live/live', update: 'disable', snapshots: false,
    providers: { 'asm-live': { name: 'ASM local verification', env: ['ASM_LIVE_KEY'],
      package: '@opencode/ai/providers/openai-compatible', settings: { baseURL: `http://127.0.0.1:${model.address().port}/v1` },
      models: { live: { name: 'ASM Live Model', limit: { context: 100000, output: 1000 } } } } },
  }));
  native = spawn(bin, ['serve', '--hostname', '127.0.0.1', '--port', '0'], { cwd: project, env, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  let errors = '';
  native.stderr.on('data', d => { errors += d; });
  const url = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(Error(`OpenCode server startup timed out: ${errors}`)), 20000);
    native.on('error', error => { clearTimeout(timer); reject(error); });
    native.on('exit', code => { clearTimeout(timer); reject(Error(`OpenCode server exited (${code}): ${errors}`)); });
    native.stdout.on('data', d => {
      output += d;
      const match = output.match(/server listening on (http:\/\/[^\s]+)/);
      if (match) { clearTimeout(timer); resolve(match[1]); }
    });
  });
  const args = ['run', '--server', url, '--auto', '--format', 'json'];
  const first = await run(bin, [...args, '--model', 'asm-live/live', '--title', 'ASM v2 actual runtime', 'Read example.txt and explain it.']);
  const events = first.stdout.trim().split('\n').map(line => JSON.parse(line));
  const id = events.find(e => e.sessionID)?.sessionID;
  assert.ok(id);
  assert.ok(first.stdout.includes('Actual OpenCode v2 integration fixture'));
  for (const extra of [[], ['--fork']]) {
    const continued = await run(bin, [...args, '--session', id, ...extra, 'Continue the ASM compatibility check.']);
    assert.ok(continued.stdout.includes('ASM OpenCode v2 live answer'), continued.stderr);
  }
  assert.equal(requests, 4);
  const db = (await run(bin, ['debug', 'paths', 'db'])).stdout.trim();
  await stopNative();
  const exported = path.join(root, 'fixture.json');
  const checked = await run('cargo', ['test', '--offline', '--locked', 'installed_v2_runtime_database_if_requested', '--', '--nocapture'], {
    cwd: path.join(repo, 'src-tauri'), timeout: 180000,
    env: { ...env, ASM_OPENCODE_LIVE_DB: db, ASM_OPENCODE_LIVE_EXPORT: exported },
  });
  assert.match(checked.stdout, /2 passed/);
  console.log(`PASS: ${version} created, continued and forked real sessions; readonly catalog/detail/search/tools and usage (4 requests, 440 tokens)`);
  const fixture = JSON.parse(readFileSync(exported, 'utf8'));
  browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const pageErrors = [];
  page.on('pageerror', e => pageErrors.push(e.message));
  await page.addInitScript(fixture => {
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      if (cmd === 'list_agents') return [{ id: 'opencode', displayName: 'OpenCode', installed: true, supportsFork: true, deletePolicy: 'permanent' }];
      if (cmd === 'catalog') return structuredClone(fixture.catalog);
      if (cmd === 'read_session') return structuredClone(fixture.details.find(d => d.meta.sessionId === args.sessionId));
      if (cmd === 'read_tool_payload') return structuredClone(fixture.payloads[args.token]);
      return null;
    } };
  }, fixture);
  await page.goto(baseURL);
  await page.locator('.session-card').first().waitFor();
  assert.equal(await page.locator('.session-card').count(), 2);
  assert.equal(await page.locator('.session-card .model-label').first().textContent(), 'ASM Live Model');
  for (let index = 0; index < 2; index++) {
    await page.locator('.session-card').nth(index).click();
    await page.locator('.detail-body').getByText('ASM OpenCode v2 live answer', { exact: true }).first().waitFor();
    await page.getByRole('button', { name: '工具调用 · read · 已完成', exact: true }).click();
    await page.locator('.collapsible.tool .collapsible-body').getByText(/Actual OpenCode v2 integration fixture/).waitFor();
    await page.keyboard.press('Escape');
  }
  assert.deepEqual(pageErrors, []);
  console.log(`PASS: actual ${version} database renders both sessions, model names and deferred tool results (${engine})`);
} finally {
  await browser?.close();
  await stopNative();
  model.closeAllConnections();
  await new Promise(resolve => model.close(resolve));
  rmSync(root, { recursive: true, force: true });
}
