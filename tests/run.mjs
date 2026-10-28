import { readdirSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import path from 'node:path';
import { createServer } from 'vite';
import { root } from './helpers/browser.mjs';
import { availablePort } from './helpers/server.mjs';

const [suite = 'e2e', ...selected] = process.argv.slice(2);
if (!['e2e', 'integration', 'manual'].includes(suite)) throw new Error(`Unknown suite: ${suite}`);
const available = readdirSync(path.join(root, 'tests', suite)).filter(f => f.endsWith('.mjs')).sort();
const names = selected.map(n => n.endsWith('.mjs') ? n : `${n}.mjs`);
for (const name of names) if (!available.includes(name)) throw new Error(`Unknown ${suite} test: ${name}`);
if (suite === 'manual' && !names.length) throw new Error('Select a local-data check: npm run test:manual -- real|codex-history');
const files = names.length ? names : available;
process.chdir(root);
let server;
let child;
let stopped;
const interrupt = signal => { stopped = signal; child?.kill(signal); };
const onInterrupt = () => interrupt('SIGINT');
const onTerminate = () => interrupt('SIGTERM');
process.on('SIGINT', onInterrupt);
process.on('SIGTERM', onTerminate);
async function run(command, args, env = {}) {
  if (stopped) throw new Error(`Tests interrupted: ${stopped}`);
  child = spawn(command, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });
  const timeout = setTimeout(() => child?.kill('SIGTERM'), 300_000);
  try {
    const [code, signal] = await once(child, 'exit');
    if (code !== 0) throw new Error(`${command} ${args.join(' ')} failed (${signal ?? code})`);
  } finally { clearTimeout(timeout); child = undefined; }
}
try {
  if (suite === 'e2e' && files.includes('security.mjs')) {
    await run(process.execPath, ['node_modules/typescript/bin/tsc']);
    await run(process.execPath, ['node_modules/vite/bin/vite.js', 'build']);
  }
  let url = process.env.BASE_URL;
  if (suite !== 'integration' && files.some(f => f !== 'security.mjs') && !url) {
    // Use an ephemeral port; never reuse or stop an unrelated developer server.
    server = await createServer({ server: { host: '127.0.0.1', port: await availablePort(), strictPort: true, open: false } });
    await server.listen();
    url = `http://127.0.0.1:${server.httpServer.address().port}/`;
  }
  for (const file of files) {
    console.log(`\nRunning ${suite}/${file}`);
    await run(process.execPath, [path.join('tests', suite, file)], url ? { BASE_URL: url } : {});
  }
} finally {
  child?.kill('SIGTERM');
  await server?.close();
  process.removeListener('SIGINT', onInterrupt);
  process.removeListener('SIGTERM', onTerminate);
}
