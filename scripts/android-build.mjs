import {spawnSync} from 'node:child_process';
import {existsSync, mkdirSync, readdirSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {publishDevelopmentApk, publishReleaseApk} from './apk-artifacts.mjs';

const android = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'android');
const args = process.argv.slice(2);
if (args.length > 1 || (args.length === 1 && !['--release', '--x86_64'].includes(args[0]))) {
  throw new Error('Usage: node scripts/android-build.mjs [--release | --x86_64]');
}
const emulatorApk = args[0] === '--x86_64';
const release = emulatorApk || args[0] === '--release';
const tooling = path.join(android, 'build', 'tooling');
const env = {...process.env};
const javaName = process.platform === 'win32' ? 'java.exe' : 'java';
if (!env.JAVA_HOME || !existsSync(path.join(env.JAVA_HOME, 'bin', javaName))) {
  const javaRoot = path.join(tooling, 'java');
  const java = existsSync(javaRoot) && readdirSync(javaRoot)
    .map(name => path.join(javaRoot, name)).find(directory => existsSync(path.join(directory, 'bin', javaName)));
  if (java) { env.JAVA_HOME = java; }
}
const sdk = [env.ANDROID_HOME, env.ANDROID_SDK_ROOT, path.join(tooling, 'sdk')]
  .find(directory => directory && existsSync(path.join(directory, 'platforms')));
if (sdk) {
  env.ANDROID_HOME = sdk;
  env.ANDROID_SDK_ROOT = sdk;
  if (!env.ANDROID_NDK_HOME || !existsSync(env.ANDROID_NDK_HOME)) {
    env.ANDROID_NDK_HOME = path.join(sdk, 'ndk', '27.1.12297006');
  }
}
env.GRADLE_USER_HOME ||= path.join(tooling, 'gradle-cache');
const pathKey = Object.keys(env).find(key => key.toLowerCase() === 'path');
const executablePaths = [env.JAVA_HOME && path.join(env.JAVA_HOME, 'bin')];
if (process.platform === 'win32') {
  const windows = env.SystemRoot || 'C:\\Windows';
  executablePaths.push(path.join(windows, 'System32'), windows);
  // Java's local socket paths cannot use the long packaged-app TEMP directory.
  const temporary = path.join(android, 'build', 'tmp');
  mkdirSync(temporary, {recursive: true});
  env.TEMP = temporary;
  env.TMP = temporary;
}
executablePaths.push(pathKey && env[pathKey]);
if (pathKey && pathKey !== 'PATH') { delete env[pathKey]; }
env.PATH = executablePaths.filter(Boolean).join(path.delimiter);
const task = release ? 'assembleRelease' : 'assembleDebug';
const gradleArgs = [task, '--console=plain'];
if (emulatorApk) { gradleArgs.push('-PreactNativeArchitectures=x86_64'); }
const result = process.platform === 'win32'
  ? spawnSync(env.ComSpec || 'cmd.exe', ['/d', '/c', 'gradlew.bat', ...gradleArgs], {cwd: android, env, stdio: 'inherit'})
  : spawnSync(path.join(android, 'gradlew'), gradleArgs, {cwd: android, env, stdio: 'inherit'});
if (result.error) {
  throw result.error;
}
if (result.status === 0) {
  const variant = release ? 'release' : 'debug';
  const apk = path.join(android, 'app', 'build', 'outputs', 'apk', variant, `app-${variant}.apk`);
  if (!existsSync(apk)) { throw new Error('Gradle finished without producing the expected APK.'); }
  let output = apk;
  if (emulatorApk) {
    output = publishDevelopmentApk(apk);
  } else if (release) {
    output = publishReleaseApk(apk);
  }
  console.log(`APK: ${output}`);
}
process.exit(result.status ?? 1);
