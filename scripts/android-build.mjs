import {spawnSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const android = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'android');
const result = process.platform === 'win32'
  ? spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/c', 'gradlew.bat', 'assembleDebug', '--console=plain'], {cwd: android, stdio: 'inherit'})
  : spawnSync(path.join(android, 'gradlew'), ['assembleDebug', '--console=plain'], {cwd: android, stdio: 'inherit'});
if (result.error) {
  throw result.error;
}
process.exit(result.status ?? 1);
