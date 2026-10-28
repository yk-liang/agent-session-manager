// Actual Claude executable, isolated configuration, and loopback streaming model.
// Optional CODEX_REQUEST_ROOT reads existing rollouts without modifying them.
import { createServer } from 'node:http';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import { root as repo } from '../helpers/browser.mjs';

const root = mkdtempSync(path.join(tmpdir(), 'asm-requests-runtime-'));
const project = path.join(root, 'project'), config = path.join(root, 'claude');
mkdirSync(project); mkdirSync(config);
const exec = promisify(execFile);
const run = (bin, args, extra = {}) => {
  const child = exec(bin, args, { cwd: project, env: process.env, timeout: 90000, maxBuffer: 2 * 1024 * 1024, ...extra });
  child.child.stdin?.end(); return child;
};
let requests = 0;
const server = createServer(async (req, res) => {
  let data = ''; for await (const chunk of req) data += chunk;
  const body = JSON.parse(data || '{}');
  if (req.url.includes('count_tokens')) { res.end('{"input_tokens":100}'); return; }
  if (!req.url.includes('/messages')) { res.statusCode = 404; res.end(); return; }
  requests++;
  res.setHeader('content-type', 'text/event-stream');
  const message = { id: `msg_actual_${requests}`, type: 'message', role: 'assistant', model: body.model,
    content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 100, output_tokens: 0 } };
  for (const event of [
    { type: 'message_start', message },
    { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } },
    { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'ASM request timing verified.' } },
    { type: 'content_block_stop', index: 0 },
    { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 25 } },
    { type: 'message_stop' },
  ]) {
    await new Promise(resolve => setTimeout(resolve, 60));
    res.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`);
  }
  res.end();
});
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const bin = process.env.CLAUDE_BIN || 'claude';
  const version = (await run(bin, ['--version'])).stdout.trim();
  const session = randomUUID();
  const env = { ...process.env, CLAUDE_CONFIG_DIR: config, ANTHROPIC_BASE_URL: `http://127.0.0.1:${server.address().port}`,
    ANTHROPIC_API_KEY: 'local-test-only', ANTHROPIC_AUTH_TOKEN: '', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1', DISABLE_AUTOUPDATER: '1' };
  for (const turn of [0, 1]) {
    const result = await run(bin, ['-p', 'Local request event verification.', ...(turn ? ['--resume', session] : ['--session-id', session]),
      '--model', 'claude-sonnet-4-6', '--tools', '', '--setting-sources', '', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}', '--output-format', 'json'], { env });
    assert.ok(result.stdout.includes('ASM request timing verified'));
  }
  assert.equal(requests, 2);
  async function verify(agent, directory) {
    const result = await run('cargo', ['test', '--offline', '--locked', 'live_request_events_if_requested', '--', '--nocapture'], {
      cwd: path.join(repo, 'src-tauri'), env: { ...process.env, ASM_REQUEST_LIVE_ROOT: directory, ASM_REQUEST_LIVE_AGENT: agent }, timeout: 180000,
    });
    assert.match(result.stdout, /1 passed/);
    console.log(result.stdout.split('\n').find(line => line.startsWith('actual ')));
  }
  await verify('claude', path.join(config, 'projects'));
  console.log(`PASS: ${version}, two actual streaming requests, resumed session, measured transcript timing`);
  if (process.env.CODEX_REQUEST_ROOT) await verify('codex', process.env.CODEX_REQUEST_ROOT);
} finally { await new Promise(resolve => server.close(resolve)); rmSync(root, { recursive: true, force: true }); }
