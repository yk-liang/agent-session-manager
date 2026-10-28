import { mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const root = fileURLToPath(new URL('../../', import.meta.url));
export const engine = process.env.ENGINE ?? 'webkit';
if (!['webkit', 'chrome'].includes(engine)) throw new Error('ENGINE must be webkit or chrome');
export const baseURL = process.env.BASE_URL ?? 'http://127.0.0.1:1420/';
export const artifactDir = path.resolve(root, process.env.TEST_ARTIFACTS ?? `test-results/${engine}`);
mkdirSync(artifactDir, { recursive: true });
