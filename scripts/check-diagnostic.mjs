import assert from 'node:assert/strict';

// A fixed loopback origin prevents accidentally forwarding Metro or the API.
const origin = 'http://127.0.0.1:8091';
const health = await fetch(`${origin}/health`, {signal: AbortSignal.timeout(3000)});
assert.equal(health.status, 200, 'Start the health-only diagnostic process first');
assert.equal(health.headers.get('x-void-diagnostic-origin'), 'health-only', 'Unexpected diagnostic origin');
assert.deepEqual(await health.json(), {status: 'ok'});
for (const path of ['/ready', '/v1/accounts']) {
  const response = await fetch(origin + path, {signal: AbortSignal.timeout(3000)});
  assert.equal(response.status, 404, 'Diagnostic origin must exclude API routes');
}
console.log('Verified loopback health-only diagnostic origin.');
