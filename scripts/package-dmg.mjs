// Repackage an already verified app; do not rebuild or re-sign its contents.
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const [appArgument, outputArgument] = process.argv.slice(2);
if (!appArgument || !outputArgument || process.argv.length !== 4) {
  throw new Error('Usage: npm run package:dmg -- <verified.app> <new-output.dmg>');
}
if (process.platform !== 'darwin') throw new Error('DMG packaging requires macOS and a logged-in Finder session');
const app = path.resolve(appArgument), output = path.resolve(outputArgument);
if (!app.endsWith('.app') || !existsSync(app)) throw new Error('Input must be an existing .app bundle');
if (!output.endsWith('.dmg') || existsSync(output)) throw new Error('Output must be a new .dmg path; existing releases are never overwritten locally');
const config = JSON.parse(readFileSync(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
const dmg = config.bundle.macOS.dmg;
const builder = path.join(root, 'src-tauri/target/universal-apple-darwin/release/bundle/dmg/bundle_dmg.sh');
if (!existsSync(builder)) throw new Error('Run a universal Tauri DMG build once to provision the bundled create-dmg script');
const run = (command, args) => execFileSync(command, args, { cwd: root, stdio: 'inherit' });
const plist = path.join(app, 'Contents/Info.plist');
const version = execFileSync('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleShortVersionString', plist], { encoding: 'utf8' }).trim();
if (version !== config.version) throw new Error(`App version ${version} does not match configured release ${config.version}`);
run('codesign', ['--verify', '--deep', '--strict', '--all-architectures', app]);
const background = path.resolve(root, 'src-tauri', dmg.background);
run('swift', [path.join(root, 'scripts/generate-dmg-background.swift'), background]);
const staging = mkdtempSync(path.join(tmpdir(), 'asm-dmg-'));
try {
  const appName = path.basename(app);
  run('ditto', [app, path.join(staging, appName)]);
  // Deliberately omit --skip-jenkins: Finder writes the layout into the image.
  // Use the upstream off-canvas positions; Finder normalizes negative positions on remount.
  run('bash', [builder, '--volname', config.productName,
    '--volicon', path.join(root, 'src-tauri/icons/icon.icns'), '--background', background,
    '--window-pos', '200', '160', '--window-size', String(dmg.windowSize.width), String(dmg.windowSize.height),
    '--icon-size', '96', '--text-size', '13', '--hide-extension', appName,
    '--icon', appName, String(dmg.appPosition.x), String(dmg.appPosition.y),
    '--app-drop-link', String(dmg.applicationFolderPosition.x), String(dmg.applicationFolderPosition.y),
    output, staging]);
  run('hdiutil', ['verify', output]);
  console.log(`Created installer with Finder layout: ${output}`);
} finally {
  rmSync(staging, { recursive: true, force: true });
}
