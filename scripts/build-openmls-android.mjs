import {spawnSync} from 'node:child_process';
import {existsSync, mkdirSync, copyFileSync} from 'node:fs';
import {homedir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {openMlsHostFlags, openMlsHostEnvironment} from './openmls-host-flags.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const crate = path.join(root, 'native', 'openmls');
const cargoHome = process.env.CARGO_HOME || path.join(homedir(), '.cargo');
const cargoCandidate = path.join(cargoHome, 'bin', process.platform === 'win32' ? 'cargo.exe' : 'cargo');
const cargo = existsSync(cargoCandidate) ? cargoCandidate : 'cargo';
const architectures = (process.argv[2] || 'arm64-v8a').split(',');
const targets = {
  'arm64-v8a': 'aarch64-linux-android',
  'armeabi-v7a': 'armv7-linux-androideabi',
  x86: 'i686-linux-android',
  x86_64: 'x86_64-linux-android',
};
const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT ||
  (process.platform === 'win32' ? path.join(homedir(), 'AppData', 'Local', 'Android', 'Sdk') :
    process.platform === 'darwin' ? path.join(homedir(), 'Library', 'Android', 'sdk') : path.join(homedir(), 'Android', 'Sdk'));
const ndk = process.env.ANDROID_NDK_HOME || path.join(sdk, 'ndk', '27.1.12297006');
const host = process.platform === 'win32' ? 'windows-x86_64' : process.platform === 'darwin' ? 'darwin-x86_64' : 'linux-x86_64';
const tools = path.join(ndk, 'toolchains', 'llvm', 'prebuilt', host, 'bin');
const generated = path.join(root, 'android', 'app', 'build', 'generated', 'openmls');
const hostEnvironment = openMlsHostEnvironment();

function run(args, env = hostEnvironment) {
  const result = spawnSync(cargo, args, {cwd: crate, env, stdio: 'inherit'});
  if (result.error) {
    throw new Error(`Rust toolchain unavailable: ${result.error.message}. Install Rust using rustup.`);
  }
  if (result.status !== 0) {
    throw new Error(`Cargo exited with ${result.status}; see its diagnostic output above.`);
  }
}

if (!existsSync(tools)) {
  throw new Error(`Android NDK 27.1.12297006 not found at ${ndk}. Set ANDROID_NDK_HOME to the installed NDK.`);
}

let library;
for (const architecture of architectures) {
  const target = targets[architecture];
  if (!target) {
    throw new Error(`Unsupported Android architecture: ${architecture}`);
  }
  const clangTarget = architecture === 'armeabi-v7a' ? 'armv7a-linux-androideabi' : target;
  const linker = path.join(tools, `${clangTarget}24-clang${process.platform === 'win32' ? '.cmd' : ''}`);
  const key = target.toUpperCase().replaceAll('-', '_');
  const env = {...hostEnvironment, [`CARGO_TARGET_${key}_LINKER`]: linker};
  // The Rust library must also load on Android devices using 16 KiB pages.
  run(['build', '--locked', '--manifest-path', path.join(crate, 'Cargo.toml'), '--release', '--lib',
    '--target', target, '--config', `target.${target}.rustflags=["-C","link-arg=-Wl,-z,max-page-size=16384"]`], env);
  library = path.join(crate, 'target', target, 'release', 'libvoid_openmls.so');
  const destination = path.join(generated, 'jniLibs', architecture);
  mkdirSync(destination, {recursive: true});
  copyFileSync(library, path.join(destination, 'libvoid_openmls.so'));
}

const kotlin = path.join(generated, 'kotlin');
mkdirSync(kotlin, {recursive: true});
run(['run', '--locked', ...openMlsHostFlags(), '--manifest-path', path.join(crate, 'Cargo.toml'), '--features', 'bindgen',
  '--bin', 'uniffi-bindgen', '--', 'generate', library, '--language', 'kotlin', '--out-dir', kotlin, '--no-format']);
