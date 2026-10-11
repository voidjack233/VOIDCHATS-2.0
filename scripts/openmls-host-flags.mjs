import {existsSync, mkdirSync, copyFileSync} from 'node:fs';
import {homedir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

function ndkDlltool() {
  const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT || path.join(homedir(), 'AppData', 'Local', 'Android', 'Sdk');
  const ndk = process.env.ANDROID_NDK_HOME || path.join(sdk, 'ndk', '27.1.12297006');
  return path.join(ndk, 'toolchains', 'llvm', 'prebuilt', 'windows-x86_64', 'bin', 'llvm-dlltool.exe');
}

export function openMlsHostEnvironment() {
  if (process.platform !== 'win32' || !existsSync(ndkDlltool())) {
    return process.env;
  }
  // Host proc macros also need dlltool during Android cross compilation.
  // Cargo does not apply target rustflags to these host build dependencies.
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const directory = path.join(root, 'native', 'openmls', 'target', 'host-tools');
  mkdirSync(directory, {recursive: true});
  const destination = path.join(directory, 'dlltool.exe');
  if (!existsSync(destination)) {
    copyFileSync(ndkDlltool(), destination);
  }
  return {...process.env, PATH: `${directory}${path.delimiter}${process.env.PATH || ''}`};
}

/** Windows GNU Rust needs dlltool for raw DLL imports. The installed NDK provides it. */
export function openMlsHostFlags() {
  if (process.platform !== 'win32') {
    return [];
  }
  const dlltool = ndkDlltool();
  if (!existsSync(dlltool)) {
    return [];
  }
  // This target-specific setting does not alter MSVC or Android builds.
  return ['--config', `target.x86_64-pc-windows-gnu.rustflags=${JSON.stringify(['-C', `dlltool=${dlltool}`])}`];
}
