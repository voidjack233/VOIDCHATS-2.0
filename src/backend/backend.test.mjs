import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import ts from 'typescript';

function loadModule(name, development = true, globals = {}) {
  const module = {exports: {}};
  const source = readFileSync(new URL(`./${name}.ts`, import.meta.url), 'utf8');
  const {outputText} = ts.transpileModule(source, {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022},
  });
  runInNewContext(outputText, {
    module, exports: module.exports, __DEV__: development, Error,
    AbortController, setTimeout, clearTimeout,
    require: name => {
      assert.equal(name, './config');
      return loadModule('config', development);
    },
    ...globals,
  });
  return module.exports;
}

test('development accepts LAN and HTTPS origins without a bundled default', () => {
  const config = loadModule('config');
  assert.equal(config.getDevelopmentBackendConfig(), null);
  for (const [input, expected] of [
    [' http://192.0.2.12:8080/ ', 'http://192.0.2.12:8080'],
    ['https://Development.Example.test', 'https://development.example.test'],
    ['http://localhost:8080', 'http://localhost:8080'],
  ]) {
    assert.equal(config.setDevelopmentBackendBaseURL(input).baseURL, expected);
    assert.equal(config.getDevelopmentBackendConfig().baseURL, expected);
  }
});

test('configuration rejects unsafe or ambiguous URL components', () => {
  const config = loadModule('config');
  for (const input of [
    '', 'host:8080', 'ftp://host', 'https://user:secret@host',
    'https://host/health', 'https://host?token=secret', 'https://host#secret',
    'http://host:0', 'http://host:65536', 'http://999.0.0.1',
    'http://-invalid.test', 'http://host with spaces',
  ]) {
    assert.throws(() => config.createBackendConfig(input), Error);
  }
});

test('release requires HTTPS and cannot enable the development selector', () => {
  const config = loadModule('config', false);
  assert.throws(() => config.createBackendConfig('http://localhost:8080'), /HTTPS/);
  assert.equal(config.createBackendConfig('https://example.test').baseURL, 'https://example.test');
  assert.throws(() => config.setDevelopmentBackendBaseURL('https://example.test'), /development builds/);
  assert.equal(config.getDevelopmentBackendConfig(), null);
});

test('diagnostic sends only GET health with no credentials or account body', async () => {
  let requested = false;
  const health = loadModule('health', true, {
    fetch: async (url, options) => {
      requested = true;
      assert.equal(url, 'https://example.test/health');
      assert.equal(options.method, 'GET');
      assert.equal(options.credentials, 'omit');
      assert.equal(options.body, undefined);
      return new Response('{"status":"ok"}', {headers: {'content-type': 'application/json'}});
    },
  });
  const result = await health.checkBackendHealth({baseURL: 'https://example.test'});
  assert.equal(requested, true);
  assert.equal(result.ok, true);
  assert.equal(result.status, 200);
});

test('diagnostic rejects tunnel login pages and unexpected health responses', async () => {
  for (const response of [
    new Response('blocked', {status: 403}),
    new Response('<html>Sign in</html>', {headers: {'content-type': 'text/html'}}),
    new Response('{"status":"wrong"}', {headers: {'content-type': 'application/json'}}),
  ]) {
    const health = loadModule('health', true, {fetch: async () => response});
    const result = await health.checkBackendHealth({baseURL: 'https://example.test'});
    assert.equal(result.ok, false);
    assert.notEqual(result.message, '');
  }
});

test('diagnostic deadline aborts the request and returns a useful timeout', async () => {
  let aborted = false;
  const health = loadModule('health', true, {
    setTimeout: callback => {queueMicrotask(callback); return 1;},
    clearTimeout: () => {},
    fetch: (_, options) => new Promise((_, reject) => {
      options.signal.addEventListener('abort', () => {
        aborted = true;
        reject(new Error('aborted'));
      });
    }),
  });
  const result = await health.checkBackendHealth({baseURL: 'https://example.test'});
  assert.equal(aborted, true);
  assert.equal(result.ok, false);
  assert.match(result.message, /Timed out after 8 seconds/);
});

test('release diagnostic cannot issue a network request', async () => {
  const health = loadModule('health', false, {fetch: () => assert.fail('release must not fetch')});
  const result = await health.checkBackendHealth({baseURL: 'https://example.test'});
  assert.equal(result.ok, false);
  assert.match(result.message, /development builds/);
});
