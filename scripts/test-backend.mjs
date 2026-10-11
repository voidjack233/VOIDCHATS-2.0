import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync, writeFileSync} from 'node:fs';

const args = process.argv.slice(2);
let originArgument;
let healthOnly = false;
let recordPath;
let verifyPath;
for (let index = 0; index < args.length; index++) {
  const argument = args[index];
  if (argument === '--health-only') {
    healthOnly = true;
  } else if (argument === '--record' || argument === '--verify-record') {
    const path = args[++index];
    if (!path || path.startsWith('--')) { throw new Error('A fixture option requires a file path.'); }
    if (argument === '--record') { recordPath = path; } else { verifyPath = path; }
  } else if (argument.startsWith('--') || originArgument !== undefined) {
    throw new Error('Use one backend origin and the documented verification options.');
  } else {
    originArgument = argument;
  }
}
if ((healthOnly && (recordPath || verifyPath)) || (recordPath && verifyPath)) {
  throw new Error('Health-only, fixture creation, and fixture verification are separate modes.');
}
let base;
try {
  base = new URL(originArgument ?? 'http://127.0.0.1:8080');
} catch {
  throw new Error('Use a valid HTTP or HTTPS backend origin.');
}
if (base.username || base.password || base.search || base.hash || base.pathname !== '/' || !['http:', 'https:'].includes(base.protocol)) {
  throw new Error('Use an HTTP or HTTPS origin without credentials, a path, query parameters, or fragments.');
}
const origin = base.origin;
async function request(method, path, body) {
  const response = await fetch(origin + path, {
    method,
    headers: {'Content-Type': 'application/json'},
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(7000),
  });
  assert.match(response.headers.get('content-type') || '', /^application\/json/);
  return {status: response.status, body: await response.json()};
}
const health = await request('GET', '/health');
assert.equal(health.status, 200);
assert.deepEqual(health.body, {status: 'ok'});
console.log('GET /health: 200, expected JSON.');
if (verifyPath) {
  const record = JSON.parse(readFileSync(verifyPath, 'utf8'));
  assert.equal(typeof record.account?.void_id, 'string', 'Fixture must contain a public account');
  const stored = await request('GET', `/v1/accounts/${encodeURIComponent(record.account.void_id)}`);
  assert.equal(stored.status, 200);
  assert.deepEqual(stored.body, record.account);
  console.log('Recorded public account survived restart with all fields unchanged.');
} else if (!healthOnly) {
  const ready = await request('GET', '/ready');
  assert.equal(ready.status, 200);
  const fixture = {void_id: `dev-check-${randomUUID()}`, display_name: 'Development fixture', identity_public_key: 'development-public-key-fixture'};
  const created = await request('POST', '/v1/accounts', fixture);
  assert.equal(created.status, 201);
  for (const field of Object.keys(fixture)) { assert.equal(created.body[field], fixture[field]); }
  assert.match(created.body.created_at, /Z$/);
  const duplicate = await request('POST', '/v1/accounts', {...fixture, display_name: 'Must not replace original'});
  assert.equal(duplicate.status, 409);
  assert.deepEqual(duplicate.body, {error: 'void_id already exists'});
  const stored = await request('GET', `/v1/accounts/${encodeURIComponent(fixture.void_id)}`);
  assert.equal(stored.status, 200);
  assert.deepEqual(stored.body, created.body);
  const missing = await request('GET', `/v1/accounts/dev-missing-${randomUUID()}`);
  assert.equal(missing.status, 404);
  const rejectedId = `dev-rejected-${randomUUID()}`;
  const rejected = await request('POST', '/v1/accounts', {...fixture, void_id: rejectedId, identity_private_key: 'NOT-A-REAL-KEY-must-be-rejected'});
  assert.equal(rejected.status, 400);
  assert.equal((await request('GET', `/v1/accounts/${rejectedId}`)).status, 404);
  if (recordPath) {
    writeFileSync(recordPath, JSON.stringify({origin, account: created.body}, null, 2));
  }
  console.log('Readiness, creation, retrieval, duplicate preservation, missing-account and private-field rejection passed.');
  console.log('A synthetic development fixture was retained for persistence verification.');
}
