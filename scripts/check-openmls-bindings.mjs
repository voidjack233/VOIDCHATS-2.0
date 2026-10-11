import {spawnSync} from 'node:child_process';
import {existsSync} from 'node:fs';
import {homedir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {openMlsHostFlags, openMlsHostEnvironment} from './openmls-host-flags.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const crate = path.join(root, 'native', 'openmls');
const cargoHome = process.env.CARGO_HOME || path.join(homedir(), '.cargo');
const candidate = path.join(cargoHome, 'bin', process.platform === 'win32' ? 'cargo.exe' : 'cargo');
const cargo = existsSync(candidate) ? candidate : 'cargo';

function check(result) {
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status || 1);
  }
}

check(spawnSync(cargo, ['build', '--locked', ...openMlsHostFlags(), '--lib'], {cwd: crate, env: openMlsHostEnvironment(), stdio: 'inherit'}));
const android = path.join(root, 'android');
const args = [':app:testDebugUnitTest', '--tests', 'com.voidchats.crypto.OpenMlsBindingsTest', '--console=plain'];
if (process.platform === 'win32') {
  // Fixed executable/arguments; no user-supplied command text is evaluated by cmd.
  check(spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/c', 'gradlew.bat', ...args], {cwd: android, stdio: 'inherit'}));
} else {
  check(spawnSync(path.join(android, 'gradlew'), args, {cwd: android, stdio: 'inherit'}));
}
