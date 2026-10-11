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
const result = spawnSync(cargo, ['test', '--locked', ...openMlsHostFlags()], {cwd: crate, env: openMlsHostEnvironment(), stdio: 'inherit'});
if (result.error) {
  throw result.error;
}
process.exit(result.status ?? 1);
