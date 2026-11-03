import assert from 'node:assert/strict';
import {generateKeyPairSync, randomBytes, sign, verify} from 'node:crypto';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import ts from 'typescript';

// Synthetic clients only: these tests never contact a server or open user data.
function modules(globals = {}) {
  const cache = new Map();
  const load = name => {
    if (cache.has(name)) { return cache.get(name).exports; }
    const module = {exports: {}};
    cache.set(name, module);
    const source = readFileSync(new URL('./' + name + '.ts', import.meta.url), 'utf8');
    const {outputText} = ts.transpileModule(source, {compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}});
    runInNewContext(outputText, {module, exports: module.exports, Error, Date, Set, Map, Object, JSON,
      AbortController, setTimeout, clearTimeout,
      require: path => { assert.ok(path.startsWith('./')); return load(path.slice(2)); }, ...globals});
    return module.exports;
  };
  return load;
}

const base = 'https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz';
function fixture() {
  const keys = generateKeyPairSync('ed25519');
  const publicKey = keys.publicKey.export({type: 'spki', format: 'der'}).subarray(-32).toString('base64');
  const challengeId = randomBytes(32).toString('base64url');
  const nonce = randomBytes(32).toString('base64url');
  const expiresAt = new Date(Date.now() + 120000).toISOString();
  const message = `VOID-NOTHINGNESS:register:v1\n${challengeId}\n${expiresAt}\n${publicKey}\n\n${nonce}\n`;
  const challenge = {challenge_id: challengeId, expires_at: expiresAt, message: Buffer.from(message).toString('base64')};
  const account = {void_id: '9223372036854775807', created_at: '2026-11-01T03:00:00.123456Z'};
  return {keys, publicKey, challengeId, nonce, message, challenge, account};
}
function response(body, status = 201, headers = {}) {
  return new Response(typeof body === 'string' ? body : JSON.stringify(body), {status, headers});
}
function errorCode(code) {
  return error => { assert.equal(error.name, 'AccountError'); assert.equal(error.code, code); return true; };
}
function plain(value) { return JSON.parse(JSON.stringify(value)); }

test('registration sends the deployed endpoints and an actual Ed25519 proof with exact public fields', async () => {
  const load = modules();
  const {createAccountClient} = load('client');
  const f = fixture();
  const calls = [];
  const client = createAccountClient(base + '///', async (url, options) => {
    calls.push({url, options});
    const body = JSON.parse(options.body);
    if (calls.length === 1) {
      assert.deepEqual(body, {auth_public_key: f.publicKey});
      return response(f.challenge);
    }
    assert.deepEqual(Object.keys(body).sort(), ['challenge_id', 'signature']);
    assert.equal(body.challenge_id, f.challengeId);
    assert.equal(verify(null, Buffer.from(f.message), f.keys.publicKey, Buffer.from(body.signature, 'base64')), true);
    return response(f.account);
  });
  const challenge = await client.challenge(f.publicKey);
  const signature = sign(null, Buffer.from(challenge.message, 'base64'), f.keys.privateKey).toString('base64');
  const account = await client.register(challenge.challenge_id, signature);
  assert.deepEqual(plain(account), f.account);
  assert.equal(typeof account.void_id, 'string');
  assert.deepEqual(calls.map(call => call.url), [base + '/challenges', base + '/accounts']);
  for (const {options} of calls) {
    assert.equal(options.method, 'POST');
    assert.equal(options.headers['Content-Type'], 'application/json');
    assert.equal(options.headers.Accept, 'application/json');
    assert.equal(options.headers['User-Agent'], 'VOIDCHATS/2.0');
    assert.equal(options.credentials, 'omit');
    assert.equal(options.signal.aborted, false);
  }
});

test('username validation counts Unicode code points and preserves valid UTF-8 names', () => {
  const {validateUsername, usernameBase64} = modules()('validation');
  for (const value of ['a', 'Space inside', '中文帳號', '👋'.repeat(32), 'e\u0301', 'alice\u00a0bob']) {
    assert.equal(validateUsername(value), true, JSON.stringify(value));
    assert.equal(usernameBase64(value), Buffer.from(value, 'utf8').toString('base64'));
  }
  for (const value of ['', '👋'.repeat(33), ' alice', 'alice ', '\u0085alice', 'alice\u202f', 'a\nb', 'a\u0000b', 'a\u200db', 'a\u202eb', '\ud800']) {
    assert.equal(validateUsername(value), false, JSON.stringify(value));
  }
});

test('noncanonical/wrong-size public keys are rejected before network access', async () => {
  const {createAccountClient} = modules()('client');
  let calls = 0;
  const client = createAccountClient(base, async () => { calls++; throw new Error('Network must not run.'); });
  const f = fixture();
  for (const key of ['Zh==', 'Zg', 'Zg==\n', Buffer.alloc(31).toString('base64'), Buffer.alloc(33).toString('base64')]) {
    await assert.rejects(client.challenge(key), errorCode('invalid_response'));
  }
  assert.equal(calls, 0);
});

test('base64 rejects alternate padding bits, URL encoding, whitespace and excessive input', () => {
  const {decodeBase64} = modules()('validation');
  for (const bytes of [Buffer.alloc(0), Buffer.from([102]), Buffer.from([102, 111]), Buffer.from([255, 254, 253]), Buffer.from(Array.from({length: 256}, (_, i) => i))]) {
    assert.deepEqual(Array.from(decodeBase64(bytes.toString('base64'))), Array.from(bytes));
  }
  for (const value of [undefined, null, 123, 'Zh==', 'Zm9=', '_w==', 'Zg', 'Zg=', 'Zg===', 'Zg==\n', ' Zg==', 'Zg==Zg==', 'A'.repeat(4100)]) {
    assert.throws(() => decodeBase64(value), Error, JSON.stringify(value));
  }
});

test('challenge proof must bind the exact domain, reserved empty line, key, token, expiry and trailing newline', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  const variants = [
    {...f.challenge, message: Buffer.from(f.message.replace('VOID-NOTHINGNESS:register:v1', 'OTHER:register:v1')).toString('base64')},
    {...f.challenge, message: Buffer.from(f.message.replace('\n\n', '\nZGlmZmVyZW50IG5hbWU=\n')).toString('base64')},
    {...f.challenge, message: Buffer.from(f.message.replace(f.publicKey, Buffer.alloc(32).toString('base64'))).toString('base64')},
    {...f.challenge, challenge_id: randomBytes(32).toString('base64url')},
    {...f.challenge, expires_at: new Date(Date.now() + 90000).toISOString()},
    {...f.challenge, message: Buffer.from(f.message.slice(0, -1)).toString('base64')},
    {...f.challenge, message: Buffer.from(f.message + 'extra\n').toString('base64')},
    {...f.challenge, message: Buffer.from(f.message.replace(f.nonce, 'short')).toString('base64')},
    {...f.challenge, message: Buffer.from(f.message + '非ASCII').toString('base64')},
    {...f.challenge, message: f.challenge.message + '\n'},
    {...f.challenge, expires_at: new Date(Date.now() - 1000).toISOString()},
  ];
  for (const body of variants) {
    const client = createAccountClient(base, async () => response(body));
    await assert.rejects(client.challenge(f.publicKey), errorCode('invalid_response'));
  }
});

test('challenge token and nonce must be canonical encodings of 32 bytes', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  // 43 characters with a final B has nonzero unused bits in base64url.
  const noncanonical = 'A'.repeat(42) + 'B';
  const variants = [
    {...f.challenge, challenge_id: noncanonical, message: Buffer.from(f.message.replace(f.challengeId, noncanonical)).toString('base64')},
    {...f.challenge, message: Buffer.from(f.message.replace(f.nonce, noncanonical)).toString('base64')},
  ];
  for (const body of variants) {
    const client = createAccountClient(base, async () => response(body));
    await assert.rejects(client.challenge(f.publicKey), errorCode('invalid_response'));
  }
});

test('a challenge outside the server maximum ten-minute lifetime is rejected', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  const expiresAt = new Date(Date.now() + 11 * 60000).toISOString();
  const body = {...f.challenge, expires_at: expiresAt,
    message: Buffer.from(f.message.replace(f.challenge.expires_at, expiresAt)).toString('base64')};
  const client = createAccountClient(base, async () => response(body));
  await assert.rejects(client.challenge(f.publicKey), errorCode('invalid_response'));
});

test('VOID-IDs accept the complete positive int64 range as decimal strings only', () => {
  const {isVoidId} = modules()('validation');
  for (const value of ['1', '9007199254740993', '9223372036854775807']) { assert.equal(isVoidId(value), true); }
  for (const value of ['0', '-1', '+1', '01', '1.0', '1e3', '9223372036854775808', '18446744073709551615', 123, null]) {
    assert.equal(isVoidId(value), false, JSON.stringify(value));
  }
});

test('account success rejects unsafe numeric IDs, unexpected username metadata and invalid timestamps', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  const signature = sign(null, Buffer.from(f.message), f.keys.privateKey).toString('base64');
  for (const body of [
    {...f.account, void_id: 110559210000000000}, {...f.account, void_id: '9223372036854775808'},
    {...f.account, void_id: '001'}, {...f.account, username: 'other'},
    {...f.account, created_at: 'not a timestamp'}, {...f.account, created_at: null},
  ]) {
    const client = createAccountClient(base, async () => response(body));
    await assert.rejects(client.register(f.challengeId, signature), errorCode('invalid_response'));
  }
});

test('registration rejects invalid signatures before network access', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  let calls = 0;
  const client = createAccountClient(base, async () => { calls++; return response(f.account); });
  for (const signature of ['not-base64', Buffer.alloc(63).toString('base64'), Buffer.alloc(65).toString('base64')]) {
    await assert.rejects(client.register(f.challengeId, signature), errorCode('invalid_response'));
  }
  assert.equal(calls, 0);
});

test('server errors are sanitized and Retry-After is retained without automatic POST retries', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  for (const [status, code] of [[400, 'invalid_input'], [401, 'invalid_proof'], [409, 'key_registered'], [429, 'rate_limited'], [429, 'capacity_reached'], [503, 'unavailable']]) {
    let calls = 0;
    const client = createAccountClient(base, async () => {
      calls++; return response({error: {code, message: 'SYNTHETIC-SECRET from upstream'}}, status, {'Retry-After': '17'});
    });
    await assert.rejects(client.challenge(f.publicKey), error => {
      assert.equal(error.code, code);
      assert.equal(error.retryAfterSeconds, 17);
      assert.equal(error.message.includes('SYNTHETIC-SECRET'), false);
      return true;
    });
    assert.equal(calls, 1);
  }
});

test('Retry-After HTTP dates and malformed values are handled without exposing upstream text', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  for (const value of [new Date(Date.now() + 60000).toUTCString(), '-5', 'tomorrow', '0']) {
    const client = createAccountClient(base, async () => response({error: {code: 'rate_limited'}}, 429, {'Retry-After': value}));
    await assert.rejects(client.challenge(f.publicKey), error => {
      if (value.endsWith('GMT')) { assert.ok(error.retryAfterSeconds >= 58 && error.retryAfterSeconds <= 60); }
      else { assert.equal(error.retryAfterSeconds, null); }
      return true;
    });
  }
});

test('network, malformed and oversized responses fail safely without retries', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  for (const [reply, expected] of [
    [() => { throw new Error('SYNTHETIC-SECRET socket details'); }, 'network'],
    [() => response('not json'), 'invalid_response'],
    [() => response('<html>Cloudflare</html>', 503), 'unavailable'],
    [() => response('x'.repeat(16385)), 'invalid_response'],
    [() => response(null), 'invalid_response'],
    [() => response({error: {code: 'unknown', message: 'SYNTHETIC-SECRET'}}, 500), 'unavailable'],
  ]) {
    let calls = 0;
    const client = createAccountClient(base, async () => { calls++; return reply(); });
    await assert.rejects(client.challenge(f.publicKey), error => {
      assert.equal(error.code, expected);
      assert.equal(error.message.includes('SYNTHETIC-SECRET'), false);
      return true;
    });
    assert.equal(calls, 1);
  }
});

test('an aborted request is classified as a timeout and is not resent', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  let calls = 0;
  const client = createAccountClient(base, async (_url, options) => {
    calls++;
    return new Promise((_resolve, reject) => options.signal.addEventListener('abort', () => reject(new Error('Aborted.')), {once: true}));
  }, 5);
  await assert.rejects(client.challenge(f.publicKey), errorCode('network_timeout'));
  assert.equal(calls, 1);
});

test('server URL configuration requires HTTPS and excludes credentials, query and fragment', () => {
  const {normalizeApiUrl} = modules()('client');
  assert.equal(normalizeApiUrl(base + '///'), base);
  for (const value of ['', 'http://api.voidchats.xyz', 'https://user:password@api.voidchats.xyz', base + '?token=secret', base + '#fragment', ' https://api.voidchats.xyz']) {
    assert.throws(() => normalizeApiUrl(value), errorCode('invalid_configuration'));
  }
});

function setup(overrides = {}) {
  const load = modules();
  const {AccountInitializer} = load('initialization');
  const f = fixture('Synthetic account');
  const identity = {localDeviceId: 'synthetic-device', clientHandle: 'synthetic-handle',
    credentialIdentityBase64: Buffer.from('synthetic-device').toString('base64'),
    signaturePublicKeyBase64: f.publicKey, accountVoidId: null, accountUsername: null,
    accountApiUrl: null, createdAt: 1000};
  const events = [];
  const deps = {
    client: {base,
      challenge: async publicKey => {
        events.push('challenge'); assert.equal(publicKey, f.publicKey);
        return f.challenge;
      },
      register: async (challengeId, signature) => {
        events.push('register'); assert.equal(challengeId, f.challengeId);
        assert.equal(verify(null, Buffer.from(f.message), f.keys.publicKey, Buffer.from(signature, 'base64')), true);
        return f.account;
      }},
    getIdentity: () => identity,
    prepare: async apiUrl => {
      events.push('prepare'); assert.equal(apiUrl, base); return identity;
    },
    sign: async message => {
      events.push('sign'); return sign(null, Buffer.from(message, 'base64'), f.keys.privateKey).toString('base64');
    },
    complete: async account => {
      events.push('complete');
      assert.deepEqual(plain(account), {...f.account, api_url: base, auth_public_key: f.publicKey});
    },
    onStage: stage => events.push('stage:' + stage),
    ...overrides,
  };
  return {initializer: new AccountInitializer(deps), deps, events, f, identity, AccountError: load('client').AccountError};
}

test('initialization commits the signed server account before reporting ready', async () => {
  const {initializer, events, f} = setup();
  await initializer.initialize();
  assert.deepEqual(events, ['stage:preparing', 'prepare', 'stage:challenge', 'challenge',
    'stage:signing', 'sign', 'stage:registering', 'register', 'stage:saving', 'complete', 'stage:ready']);
  assert.equal(initializer.hasPendingRegistration, false);
});

test('concurrent initialization calls share one proof and one account submission', async () => {
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const {initializer, deps, events, f, identity} = setup();
  let preparations = 0;
  deps.prepare = async () => { preparations++; await gate; return identity; };
  const first = initializer.initialize();
  const second = initializer.initialize();
  assert.equal(first, second);
  assert.equal(preparations, 1);
  release();
  await Promise.all([first, second]);
  assert.equal(events.filter(event => event === 'challenge').length, 1);
  assert.equal(events.filter(event => event === 'register').length, 1);
  assert.equal(events.filter(event => event === 'complete').length, 1);
});

test('failed local completion retries only the retained successful server result', async () => {
  const {initializer, deps, events, f} = setup();
  const accounts = [];
  deps.complete = async account => {
    accounts.push(plain(account));
    if (accounts.length === 1) { throw new Error('Synthetic disk failure.'); }
  };
  await assert.rejects(initializer.initialize(), errorCode('save_failed'));
  assert.equal(initializer.hasPendingRegistration, true);
  assert.equal(events.includes('stage:ready'), false);
  const beforeRetry = events.length;
  // Retry the received account result locally without another registration POST.
  await initializer.initialize();
  assert.deepEqual(accounts, [
    {...f.account, api_url: base, auth_public_key: f.publicKey},
    {...f.account, api_url: base, auth_public_key: f.publicKey},
  ]);
  assert.deepEqual(events.slice(beforeRetry), ['stage:saving', 'stage:ready']);
  assert.equal(events.filter(event => event === 'prepare').length, 1);
  assert.equal(events.filter(event => event === 'register').length, 1);
  assert.equal(initializer.hasPendingRegistration, false);
});

test('already registered identity restores without creating keys or sending network requests', async () => {
  const {initializer, deps, events, f, identity} = setup();
  deps.getIdentity = () => ({...identity, accountVoidId: f.account.void_id, accountUsername: 'Preserved legacy name', accountApiUrl: base});
  await initializer.initialize();
  assert.deepEqual(events, ['stage:ready']);
});

test('registered identities cannot silently switch to a different server', async () => {
  const {initializer, deps, events, f, identity} = setup();
  deps.getIdentity = () => ({...identity, accountVoidId: f.account.void_id, accountApiUrl: 'https://other.example'});
  await assert.rejects(initializer.initialize(), errorCode('invalid_configuration'));
  assert.deepEqual(events, []);
});

test('manual retry after an ambiguous account POST uses a fresh challenge and the same saved key', async () => {
  const load = modules();
  const {createAccountClient} = load('client');
  const {AccountInitializer} = load('initialization');
  const first = fixture('Persistent synthetic identity');
  const second = {...first.challenge,
    challenge_id: randomBytes(32).toString('base64url')};
  const secondMessage = first.message.replace(first.challengeId, second.challenge_id);
  second.message = Buffer.from(secondMessage).toString('base64');
  const requests = [];
  let challengeRequests = 0;
  let submissions = 0;
  const client = createAccountClient(base, async (url, options) => {
    const body = JSON.parse(options.body);
    requests.push({url, body});
    if (url.endsWith('/challenges')) {
      assert.equal(body.auth_public_key, first.publicKey);
      return response(++challengeRequests === 1 ? first.challenge : second);
    }
    if (++submissions === 1) { throw new Error('Synthetic response lost after submission.'); }
    assert.equal(verify(null, Buffer.from(secondMessage), first.keys.publicKey, Buffer.from(body.signature, 'base64')), true);
    return response(first.account);
  });
  const identity = {signaturePublicKeyBase64: first.publicKey, accountVoidId: null};
  let saves = 0;
  const initializer = new AccountInitializer({client, getIdentity: () => identity,
    prepare: async () => identity,
    sign: async message => sign(null, Buffer.from(message, 'base64'), first.keys.privateKey).toString('base64'),
    complete: async () => { saves++; }, onStage: () => {}});
  await assert.rejects(initializer.initialize(), errorCode('network'));
  assert.equal(requests.length, 2);
  assert.equal(saves, 0);
  assert.equal(initializer.hasPendingRegistration, false);
  await initializer.initialize();
  assert.equal(requests.length, 4);
  assert.deepEqual(requests.filter(request => request.url.endsWith('/accounts')).map(request => request.body.challenge_id), [first.challengeId, second.challenge_id]);
  assert.equal(saves, 1);
  assert.equal(identity.signaturePublicKeyBase64, first.publicKey);
});

test('duplicate-key responses preserve identity and never falsely complete account setup', async () => {
  const {initializer, deps, events, f, identity, AccountError} = setup();
  deps.client.register = async () => { events.push('register'); throw new AccountError('key_registered'); };
  await assert.rejects(initializer.initialize(), errorCode('key_registered'));
  assert.equal(identity.accountVoidId, null);
  assert.equal(identity.signaturePublicKeyBase64, f.publicKey);
  assert.equal(events.includes('complete'), false);
  assert.equal(events.includes('stage:ready'), false);
  assert.equal(initializer.hasPendingRegistration, false);
});

class FakeXMLHttpRequest {
  constructor(operation = () => {}) {
    this.operation = operation;
    this.listeners = new Map();
    this.requestHeaders = {};
    this.responseHeaders = {};
    this.readyState = 0;
    this.status = 0;
    this.responseText = '';
    this.withCredentials = true;
    this.timeout = 0;
    this.sendCount = 0;
    this.abortCount = 0;
  }
  addEventListener(type, listener) {
    const handlers = this.listeners.get(type) ?? new Set();
    handlers.add(listener); this.listeners.set(type, handlers);
  }
  removeEventListener(type, listener) { this.listeners.get(type)?.delete(listener); }
  emit(type) {
    const event = {type, target: this};
    this['on' + type]?.(event);
    for (const listener of this.listeners.get(type) ?? []) { listener(event); }
  }
  open(method, url, async = true) {
    this.method = method; this.url = url; this.async = async; this.readyState = 1;
  }
  setRequestHeader(name, value) { this.requestHeaders[name.toLowerCase()] = value; }
  getResponseHeader(name) { return this.responseHeaders[name.toLowerCase()] ?? null; }
  getAllResponseHeaders() { return Object.entries(this.responseHeaders).map(([key, value]) => key + ': ' + value).join('\r\n'); }
  send(body) { this.body = body; this.sendCount++; this.operation(this); }
  abort() { this.abortCount++; this.readyState = 4; this.emit('abort'); this.emit('loadend'); }
  respond(status, body, headers = {}) {
    this.status = status;
    this.responseText = typeof body === 'string' ? body : JSON.stringify(body);
    this.responseHeaders = Object.fromEntries(Object.entries(headers).map(([key, value]) => [key.toLowerCase(), value]));
    this.readyState = 4; this.emit('readystatechange'); this.emit('load'); this.emit('loadend');
  }
  fail(nativeMessage, event = 'error') {
    this.status = 0; this.responseText = nativeMessage;
    this.readyState = 4; this.emit('readystatechange'); this.emit(event); this.emit('loadend');
  }
}

function xhrClient(operation, timeoutMs) {
  const load = modules();
  const requests = [];
  const {createAccountTransport} = load('transport');
  const transport = createAccountTransport(() => {
    const request = new FakeXMLHttpRequest(operation); requests.push(request); return request;
  });
  return {load, transport, requests, client: load('client').createAccountClient(base, transport, timeoutMs)};
}

test('Android text transport sends the exact JSON request without cookies and preserves HTTP response headers', async () => {
  const f = fixture();
  const {client, requests} = xhrClient(request => request.respond(201, f.challenge, {'Content-Type': 'application/json'}));
  const result = await client.challenge(f.publicKey);
  assert.deepEqual(plain(result), f.challenge);
  assert.equal(requests.length, 1);
  const request = requests[0];
  assert.equal(request.url, base + '/challenges');
  assert.equal(request.method, 'POST');
  assert.equal(request.async, true);
  assert.equal(request.withCredentials, false);
  assert.equal(request.timeout, 20000);
  assert.equal(request.requestHeaders['content-type'], 'application/json');
  assert.equal(request.requestHeaders.accept, 'application/json');
  assert.equal(request.requestHeaders['user-agent'], 'VOIDCHATS/2.0');
  assert.deepEqual(JSON.parse(request.body), {auth_public_key: f.publicKey});
  assert.equal(request.sendCount, 1);

  const limited = xhrClient(request => request.respond(429, {error: {code: 'rate_limited', message: 'RAW-SECRET-MARKER'}}, {'Retry-After': '23'}));
  await assert.rejects(limited.client.challenge(f.publicKey), error => {
    assert.equal(error.code, 'rate_limited'); assert.equal(error.retryAfterSeconds, 23);
    assert.equal(error.message.includes('RAW-SECRET-MARKER'), false); return true;
  });
  assert.equal(limited.requests.length, 1);
});

test('Android native DNS, TLS, timeout, refusal and unavailable failures produce safe actionable errors', async () => {
  const f = fixture();
  const cases = [
    ['java.net.UnknownHostException: Unable to resolve host api.voidchats.xyz: RAW-SECRET-MARKER', 'error', 'network_dns'],
    ['javax.net.ssl.SSLHandshakeException: Trust anchor for certification path not found. RAW-SECRET-MARKER', 'error', 'network_tls'],
    ['javax.net.ssl.SSLPeerUnverifiedException: Hostname not verified. RAW-SECRET-MARKER', 'error', 'network_tls'],
    ['RAW-SECRET-MARKER', 'timeout', 'network_timeout'],
    ['java.net.ConnectException: ECONNREFUSED Connection refused. RAW-SECRET-MARKER', 'error', 'network_refused'],
    ['java.net.SocketException: ENETUNREACH Network is unreachable. RAW-SECRET-MARKER', 'error', 'network_unavailable'],
    ['Unexpected native failure RAW-SECRET-MARKER', 'error', 'network'],
  ];
  for (const [nativeMessage, event, expected] of cases) {
    const {client, requests} = xhrClient(request => request.fail(nativeMessage, event));
    await assert.rejects(client.challenge(f.publicKey), error => {
      assert.equal(error.name, 'AccountError'); assert.equal(error.code, expected);
      assert.equal(error.message.includes('RAW-SECRET-MARKER'), false);
      assert.equal(error.message.includes(nativeMessage), false);
      return true;
    });
    assert.equal(requests.length, 1, expected);
    assert.equal(requests[0].sendCount, 1, expected);
  }
});

test('Android in-flight abort cancels the sole request without resubmitting the POST', async () => {
  const {client, requests} = xhrClient(() => {}, 5);
  const f = fixture();
  await assert.rejects(client.challenge(f.publicKey), errorCode('network_timeout'));
  assert.equal(requests.length, 1);
  assert.equal(requests[0].sendCount, 1);
  assert.equal(requests[0].abortCount, 1);
});

test('Android transport allows repeated body reads without making another HTTP request', async () => {
  const {transport, requests} = xhrClient(request => request.respond(201, {public: 'response'}, {'Retry-After': '9'}));
  const abort = new AbortController();
  const result = await transport(base + '/accounts', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: '{}', signal: abort.signal, credentials: 'omit'});
  assert.equal(result.status, 201);
  assert.equal(result.headers.get('retry-after'), '9');
  assert.equal(await result.text(), '{"public":"response"}');
  assert.equal(await result.text(), '{"public":"response"}');
  abort.abort();
  assert.equal(requests.length, 1);
  assert.equal(requests[0].sendCount, 1);
  assert.equal(requests[0].abortCount, 0);
});

test('Android synchronous transport exceptions remain sanitized and never trigger automatic retries', async () => {
  const f = fixture();
  const {client, requests} = xhrClient(() => { throw new Error('RAW-SECRET-MARKER sync send failure'); });
  await assert.rejects(client.challenge(f.publicKey), error => {
    assert.equal(error.code, 'network'); assert.equal(error.message.includes('RAW-SECRET-MARKER'), false); return true;
  });
  assert.equal(requests.length, 1);
  assert.equal(requests[0].sendCount, 1);
});

test('an already aborted Android request never sends its POST body', async () => {
  const {transport, requests} = xhrClient(() => { throw new Error('An aborted request must not send.'); });
  const abort = new AbortController(); abort.abort();
  await assert.rejects(transport(base + '/accounts', {method: 'POST', headers: {}, body: '{}', signal: abort.signal, credentials: 'omit'}), errorCode('network_timeout'));
  assert.equal(requests.length, 1);
  assert.equal(requests[0].sendCount, 0);
});

test('a failing Android request factory returns a safe error instead of exposing native exception details', async () => {
  const load = modules(); const f = fixture(); let calls = 0;
  const transport = load('transport').createAccountTransport(() => { calls++; throw new Error('RAW-SECRET-MARKER constructor failed'); });
  const client = load('client').createAccountClient(base, transport);
  await assert.rejects(client.challenge(f.publicKey), error => {
    assert.equal(error.code, 'network'); assert.equal(error.message.includes('RAW-SECRET-MARKER'), false); return true;
  });
  assert.equal(calls, 1);
});

test('response body read failures do not resubmit an account request or expose native details', async () => {
  const {createAccountClient} = modules()('client');
  const f = fixture();
  let calls = 0;
  const client = createAccountClient(base, async () => {
    calls++; return {status: 201, headers: {get: () => null}, text: async () => { throw new Error('RAW-SECRET-MARKER body read failed'); }};
  });
  const signature = sign(null, Buffer.from(f.message), f.keys.privateKey).toString('base64');
  await assert.rejects(client.register(f.challengeId, signature), error => {
    assert.equal(error.code, 'network'); assert.equal(error.message.includes('RAW-SECRET-MARKER'), false); return true;
  });
  assert.equal(calls, 1);
});
