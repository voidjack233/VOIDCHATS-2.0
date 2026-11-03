import {createHash} from 'node:crypto';
import {copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, writeFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const workspace = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const build = path.join(workspace, 'build');
const artifacts = path.join(build, 'apk');
const format = 'void-android-apk-v1';
const versionPattern = /^\d+(?:\.\d+){1,3}(?:[-+][A-Za-z0-9.-]+)?$/;

function contained(target, parent) {
  const relative = path.relative(parent, target);
  if (!relative || relative === '..' || relative.startsWith('..' + path.sep) || path.isAbsolute(relative)) {
    throw new Error('APK artifact path escaped its output directory.');
  }
}
function directory(target, parent) {
  contained(path.resolve(target), path.resolve(parent));
  if (existsSync(target) && lstatSync(target).isSymbolicLink()) { throw new Error('APK output directories cannot be symlinks.'); }
  mkdirSync(target, {recursive: true});
  contained(realpathSync(target), realpathSync(parent));
}

/** Same version overwrites; retain only two managed versions, without a latest APK copy. */
export function publishApk(apk, versionName, versionCode) {
  if (typeof versionName !== 'string' || versionName.length > 64 || !versionPattern.test(versionName) ||
      !Number.isSafeInteger(versionCode) || versionCode < 1) { throw new Error('Invalid APK version metadata.'); }
  directory(build, workspace);
  directory(artifacts, build);
  const output = path.join(artifacts, 'v' + versionName);
  directory(output, artifacts);
  const source = statSync(apk);
  if (!source.isFile() || source.size === 0) { throw new Error('APK is missing or empty.'); }
  const temporary = path.join(output, 'VOID.apk.tmp');
  copyFileSync(apk, temporary);
  const sha256 = createHash('sha256').update(readFileSync(temporary)).digest('hex');
  renameSync(temporary, path.join(output, 'VOID.apk'));
  const metadata = {format, versionName, versionCode, builtAt: source.mtime.toISOString(), bytes: source.size, sha256};
  writeFileSync(path.join(output, 'VOID.apk.sha256'), `${sha256}  VOID.apk\n`);
  writeFileSync(path.join(output, 'build.json'), JSON.stringify(metadata, null, 2) + '\n');

  const versions = readdirSync(artifacts, {withFileTypes: true}).filter(entry => entry.isDirectory() && /^v\d/.test(entry.name)).flatMap(entry => {
    const folder = path.join(artifacts, entry.name);
    contained(realpathSync(folder), realpathSync(artifacts));
    try {
      const item = JSON.parse(readFileSync(path.join(folder, 'build.json'), 'utf8'));
      if (item.format !== format || !versionPattern.test(item.versionName) || entry.name !== 'v' + item.versionName ||
          !Number.isSafeInteger(item.versionCode) || !existsSync(path.join(folder, 'VOID.apk'))) { return []; }
      return [{...item, folder, name: entry.name}];
    } catch { return []; } // Leave unrelated or unrecognized folders alone.
  }).sort((left, right) => right.versionCode - left.versionCode || right.builtAt.localeCompare(left.builtAt));
  const kept = versions.slice(0, 2);
  const latest = kept[0];
  writeFileSync(path.join(artifacts, 'latest.json'), JSON.stringify({versionName: latest.versionName,
    versionCode: latest.versionCode, apk: `${latest.name}/VOID.apk`, sha256: latest.sha256}, null, 2) + '\n');
  writeFileSync(path.join(build, 'README.md'), `# Android builds\n\nLatest: [VOID ${latest.versionName} (${latest.versionCode})](apk/${latest.name}/VOID.apk)\n\n` +
    'Rebuilding a version replaces its APK. Only the newest two versions are kept.\nThe latest pointer does not duplicate the APK.\n\n' +
    kept.map((item, index) => `- ${index === 0 ? 'Latest' : 'Previous'}: [${item.versionName}](apk/${item.name}/VOID.apk) · [checksum](apk/${item.name}/VOID.apk.sha256)`).join('\n') + '\n');
  for (const old of versions.slice(2)) {
    // Recheck the resolved absolute target immediately before recursive removal.
    if (lstatSync(old.folder).isSymbolicLink()) { throw new Error('Refusing to prune a symlink.'); }
    contained(realpathSync(old.folder), realpathSync(artifacts));
    rmSync(old.folder, {recursive: true});
  }
  return path.join(output, 'VOID.apk');
}

export function publishReleaseApk(apk) {
  const metadata = JSON.parse(readFileSync(path.join(path.dirname(apk), 'output-metadata.json'), 'utf8'));
  const element = metadata.elements?.find(item => item.outputFile === path.basename(apk));
  if (!element) { throw new Error('Gradle APK version metadata is missing.'); }
  return publishApk(apk, element.versionName, element.versionCode);
}
