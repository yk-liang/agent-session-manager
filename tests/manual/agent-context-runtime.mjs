// Real Claude Code and Pi executables, isolated storage, and a loopback model server.
// Requires claude/pi on PATH, or CLAUDE_BIN / PI_BIN. No provider credentials are used.
import { createServer } from 'node:http';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createInterface } from 'node:readline';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, mkdirSync, writeFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import { root as repo } from '../helpers/browser.mjs';

const root = mkdtempSync(path.join(tmpdir(), 'asm-context-runtime-'));
const project = path.join(root, 'project'), claudeConfig = path.join(root, 'claude'), piConfig = path.join(root, 'pi');
const exported = path.join(root, 'export');
for (const dir of [project, claudeConfig, piConfig, exported]) mkdirSync(dir, { recursive: true });
const exec = promisify(execFile);
async function run(bin, args, extra = {}) {
  const task = exec(bin, args, { cwd: project, env: process.env, timeout: 90000, maxBuffer: 2 * 1024 * 1024, ...extra });
  task.child.stdin?.end();
  return task;
}
const requests = { claude: 0, pi: 0 };
const model = createServer(async (req, res) => {
  let raw = ''; for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw || '{}');
  if (req.url.includes('count_tokens')) { res.setHeader('content-type', 'application/json'); res.end('{"input_tokens":100}'); return; }
  if (req.url.includes('/messages')) {
    requests.claude++;
    const message = { id: `msg_fixture_${requests.claude}`, type: 'message', role: 'assistant',
      content: [{ type: 'text', text: 'ASM actual Claude answer' }], model: body.model,
      stop_reason: 'end_turn', stop_sequence: null, usage: { input_tokens: 100, output_tokens: 5 } };
    if (!body.stream) { res.setHeader('content-type', 'application/json'); res.end(JSON.stringify(message)); return; }
    res.setHeader('content-type', 'text/event-stream');
    for (const event of [
      { type: 'message_start', message: { ...message, content: [], stop_reason: null } },
      { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } },
      { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'ASM actual Claude answer' } },
      { type: 'content_block_stop', index: 0 },
      { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 5 } },
      { type: 'message_stop' },
    ]) res.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`);
    res.end();
  } else {
    requests.pi++;
    res.setHeader('content-type', 'text/event-stream');
    for (const [delta, finish] of [[{ role: 'assistant', content: 'ASM actual Pi answer' }, null], [{}, 'stop']]) {
      res.write(`data: ${JSON.stringify({ id: 'chatcmpl-fixture', object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000),
        model: 'live', choices: [{ index: 0, delta, finish_reason: finish }],
        ...(finish ? { usage: { prompt_tokens: 100, completion_tokens: 5, total_tokens: 105 } } : {}) })}\n\n`);
    }
    res.end('data: [DONE]\n\n');
  }
});
let pi;
try {
  const claudeBin = process.env.CLAUDE_BIN || 'claude', piBin = process.env.PI_BIN || 'pi';
  const claudeVersion = (await run(claudeBin, ['--version'])).stdout.trim();
  const piVersionResult = await run(piBin, ['--version']);
  const piVersion = (piVersionResult.stdout || piVersionResult.stderr).trim();
  assert.match(piVersion, /\d+\.\d+\.\d+/);
  await new Promise(resolve => model.listen(0, '127.0.0.1', resolve));
  const endpoint = `http://127.0.0.1:${model.address().port}`;
  writeFileSync(path.join(project, 'CLAUDE.md'), 'ASM_PROJECT_CONTEXT: Disposable parser fixture.\n');
  const result = await run(claudeBin, ['-p', 'ASM genuine user request', '--session-id', randomUUID(),
    '--model', 'claude-sonnet-4-6', '--tools', '', '--setting-sources', '', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}',
    '--append-system-prompt', 'ASM_SYSTEM_CONTEXT: Parser verification.', '--system-prompt-snapshot', 'on',
    '--exclude-dynamic-system-prompt-sections', '--output-format', 'json'], {
    env: { ...process.env, CLAUDE_CONFIG_DIR: claudeConfig, ANTHROPIC_API_KEY: 'local-test-only', ANTHROPIC_AUTH_TOKEN: '',
      ANTHROPIC_BASE_URL: endpoint, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1', DISABLE_AUTOUPDATER: '1' },
  });
  assert.ok(result.stdout.includes('ASM actual Claude answer'));
  console.log(`PASS: ${claudeVersion} produced a real transcript with environment attachments and a system prompt snapshot`);

  writeFileSync(path.join(piConfig, 'models.json'), JSON.stringify({ providers: { 'asm-local': { baseUrl: endpoint + '/v1',
    api: 'openai-completions', apiKey: 'local-test-only', models: [{ id: 'live', contextWindow: 100000, maxTokens: 1000 }] } } }));
  writeFileSync(path.join(piConfig, 'settings.json'), JSON.stringify({ compaction: { enabled: false, keepRecentTokens: 0, reserveTokens: 2000 } }));
  const extension = path.join(root, 'extension.ts');
  writeFileSync(extension, `export default function(pi) { pi.on('before_agent_start', async () => ({message:{customType:'asm-verification',content:'ASM_PI_CONTEXT: injected fixture',display:false}})); }`);
  pi = spawn(piBin, ['--offline', '--mode', 'rpc', '--provider', 'asm-local', '--model', 'live', '--no-tools',
    '--no-extensions', '--extension', extension, '--no-skills', '--no-context-files', '--no-prompt-templates', '--no-themes'], {
    cwd: project, env: { ...process.env, PI_CODING_AGENT_DIR: piConfig, PI_TELEMETRY: '0' }, stdio: ['pipe', 'pipe', 'pipe'],
  });
  let errors = ''; pi.stderr.on('data', data => { errors += data; });
  const lines = createInterface({ input: pi.stdout });
  await new Promise((resolve, reject) => {
    let completed = false;
    const timer = setTimeout(() => reject(Error(`Pi timed out: ${errors}`)), 60000);
    pi.once('error', error => { clearTimeout(timer); reject(error); });
    pi.once('exit', code => { if (!completed) { clearTimeout(timer); reject(Error(`Pi exited ${code}: ${errors}`)); } });
    lines.on('line', line => {
      let event; try { event = JSON.parse(line); } catch { return; }
      if (event.type === 'agent_end') pi.stdin.write('{"id":"compact","type":"compact"}\n');
      if (event.type === 'response' && event.id === 'compact') {
        completed = true; clearTimeout(timer);
        event.success ? resolve() : reject(Error(JSON.stringify(event)));
      }
    });
    pi.stdin.write('{"id":"prompt","type":"prompt","message":"ASM genuine Pi request"}\n');
  });
  pi.kill('SIGTERM');
  await new Promise(resolve => pi.exitCode !== null ? resolve() : pi.once('exit', resolve));
  console.log(`PASS: Pi ${piVersion} persisted real dialogue, extension context and a completed compaction`);
  assert.equal(requests.claude, 1); assert.equal(requests.pi, 2);
  const logs = dir => readdirSync(dir, { recursive: true }).filter(name => name.endsWith('.jsonl')).map(name => path.join(dir, name));
  const claudeLogs = logs(path.join(claudeConfig, 'projects')), piLogs = logs(path.join(piConfig, 'sessions'));
  assert.equal(claudeLogs.length, 1); assert.equal(piLogs.length, 1);
  const checked = await run('cargo', ['test', '--offline', '--locked', 'context_visibility_actual_', '--', '--test-threads=1'], {
    cwd: path.join(repo, 'src-tauri'), timeout: 180000,
    env: { ...process.env, ASM_FAMILY_CONTEXT_LIVE: claudeLogs[0], ASM_PI_CONTEXT_LIVE: piLogs[0], ASM_AGENT_CONTEXT_EXPORT: exported },
  });
  assert.match(checked.stdout, /2 passed/);
  const ui = await run(process.execPath, ['tests/e2e/agent-context.mjs'], {
    cwd: repo, timeout: 180000, env: { ...process.env, ASM_AGENT_CONTEXT_LIVE_EXPORT: exported },
  });
  process.stdout.write(ui.stdout);
} finally { pi?.kill('SIGTERM'); model.close(); rmSync(root, { recursive: true, force: true }); }
