import assert from 'node:assert/strict';
import {generateKeyPairSync, randomBytes, sign, verify} from 'node:crypto';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import ts from 'typescript';

// Synthetic socket and native fixtures only. No backend or user data is touched.
function modules() {
  const cache = new Map();
  const load = url => {
    if (cache.has(url.href)) { return cache.get(url.href).exports; }
    const module = {exports: {}}; cache.set(url.href, module);
    const {outputText} = ts.transpileModule(readFileSync(url, 'utf8'), {compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}});
    runInNewContext(outputText, {module, exports: module.exports, Error, Date, Set, Map, Object, JSON,
      setTimeout, clearTimeout, AbortController,
      require: path => load(new URL(path + '.ts', url))});
    return module.exports;
  };
  return name => load(new URL('./' + name + '.ts', import.meta.url));
}
const base = 'https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz';
const eventId = '11111111-1111-4111-8111-111111111111';
const peerId = '9223372036854775806';
const b64 = value => Buffer.from(value).toString('base64');
const plain = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
async function until(condition) { for (let count = 0; count < 100; count++) { if (condition()) { return; } await tick(); } assert.fail('Condition was not reached'); }
function fixture(overrides = {}) {
  const keys = generateKeyPairSync('ed25519');
  const publicKey = keys.publicKey.export({type: 'spki', format: 'der'}).subarray(-32).toString('base64');
  const identity = {localDeviceId: 'synthetic-device', accountVoidId: '9223372036854775807', accountApiUrl: base, signaturePublicKeyBase64: publicKey};
  const challengeId = randomBytes(32).toString('base64url');
  const expiry = new Date(Date.now() + 10000).toISOString();
  const fields = ['VOID-NOTHINGNESS:authenticate:v2', challengeId, identity.accountVoidId, publicKey, expiry, b64(base), randomBytes(32).toString('base64url'), ''];
  if (overrides.fields) { overrides.fields(fields); }
  const challenge = {type: 'auth_challenge', challenge_id: challengeId, message: b64(fields.join('\n')), expires_at: expiry, ...overrides.challenge};
  const sent = []; const events = []; const disconnected = []; let signed = 0; let closed = 0;
  const socket = {onopen: null, onmessage: null, onerror: null, onclose: null,
    close() { closed++; }, send(text) {
      const frame = JSON.parse(text); sent.push(frame);
      if (frame.type === 'auth_begin') { socket.emit(challenge); }
      else if (frame.type === 'auth_proof') {
        assert.equal(verify(null, Buffer.from(challenge.message, 'base64'), keys.publicKey, Buffer.from(frame.signature, 'base64')), true);
        socket.emit({type: 'auth_ok', void_id: identity.accountVoidId});
      } else { overrides.command?.(frame, socket); }
    }, emit(frame) { socket.onmessage?.({data: JSON.stringify(frame)}); },
  };
  const {MessagingClient} = modules()('client');
  const client = new MessagingClient({apiUrl: base, identity: () => identity,
    socketFactory: url => { assert.equal(url, base.replace('https:', 'wss:') + '/ws'); queueMicrotask(() => socket.onopen?.()); return socket; },
    signAuthChallenge: async message => { signed++; if (overrides.sign) { return overrides.sign(message); } return sign(null, Buffer.from(message, 'base64'), keys.privateKey).toString('base64'); },
    onEvent: event => events.push(event), onDisconnect: error => disconnected.push(error.code), timeoutMs: overrides.timeoutMs ?? 1000,
  });
  return {client, socket, sent, events, disconnected, identity, publicKey, get signed() { return signed; }, get closed() { return closed; }};
}

test('v2 authentication signs the exact bound challenge, then lookup uses the authenticated socket', async t => {
  const f = fixture({command(frame, socket) { assert.deepEqual(frame, {type: 'lookup', void_id: peerId}); socket.emit({type: 'person', void_id: peerId, auth_public_key: f.publicKey}); }});
  t.after(() => f.client.reset());
  const peer = await f.client.lookup(peerId);
  assert.deepEqual(plain(peer), {void_id: peerId, auth_public_key: f.publicKey});
  assert.deepEqual(f.sent.map(frame => frame.type), ['auth_begin', 'auth_proof', 'lookup']);
  assert.equal(f.signed, 1);
  assert.equal('username' in peer, false);
});
for (const [name, change] of [
  ['domain', f => { f[0] = 'VOID-NOTHINGNESS:auth:v1'; }],
  ['account', f => { f[2] = peerId; }],
  ['key', f => { f[3] = b64(Buffer.alloc(32)); }],
  ['audience', f => { f[5] = b64('https://elsewhere.invalid'); }],
  ['nonce', f => { f[6] = 'unbound'; }],
  ['extra line', f => { f.push('extra'); }],
  ['expiry binding', f => { f[4] = new Date(Date.now() + 10000).toISOString() + 'x'; }],
]) {
  test('rejects a mismatched ' + name + ' before invoking the signer', async t => {
    const f = fixture({fields: change}); t.after(() => f.client.reset());
    await assert.rejects(f.client.connect(), error => error.code === 'invalid_response');
    assert.equal(f.signed, 0); assert.equal(f.closed, 1);
  });
}
test('expired challenge and excessive TTL never reach the protected signer', async () => {
  for (const expires_at of [new Date(Date.now() - 1000).toISOString(), new Date(Date.now() + 60000).toISOString()]) {
    const f = fixture({challenge: {expires_at}});
    await assert.rejects(f.client.connect()); assert.equal(f.signed, 0); f.client.reset();
  }
});
test('reset during signing prevents a stale proof reaching a replacement connection', async () => {
  let release;
  const f = fixture({sign: () => new Promise(resolve => { release = resolve; })});
  const connecting = f.client.connect();
  await until(() => f.signed === 1); f.client.reset(); release(b64(Buffer.alloc(64)));
  await assert.rejects(connecting); await tick();
  assert.deepEqual(f.sent.map(frame => frame.type), ['auth_begin']);
});
test('reset immediately after auth_ok cannot send an old command on a new connection', async () => {
  const f = fixture();
  const originalEmit = f.socket.emit;
  f.socket.emit = frame => { originalEmit(frame); if (frame.type === 'auth_ok') { f.client.reset(); } };
  await assert.rejects(f.client.lookup(peerId));
  assert.equal(f.sent.some(frame => frame.type === 'lookup'), false);
});
test('commands serialize uncorrelated errors while incoming events interleave', async t => {
  const commands = [];
  const f = fixture({command(frame, socket) {
    commands.push(frame);
    if (commands.length === 2) { socket.emit({type: 'person', void_id: '123', auth_public_key: f.publicKey}); }
  }}); t.after(() => f.client.reset());
  const first = f.client.lookup(peerId); const rejected = assert.rejects(first, error => error.code === 'not_found');
  const second = f.client.lookup('123');
  await until(() => commands.length === 1); await tick(); assert.equal(commands.length, 1);
  f.socket.emit({type: 'online_event', exchange_id: 'public.exchange', from_void_id: peerId, payload_base64: b64('opaque')});
  f.socket.emit({type: 'error', code: 'not_found'});
  await rejected; assert.equal((await second).void_id, '123'); assert.equal(f.events.length, 1);
});
test('accepted means server acceptance only, and duplicate_event remains an error', async t => {
  const created_at = new Date().toISOString(); const expires_at = new Date(Date.now() + 100000).toISOString();
  let count = 0;
  const f = fixture({command(frame, socket) {
    assert.deepEqual(frame, {type: 'send', event_id: eventId, to_void_id: peerId, payload_base64: b64('opaque')});
    socket.emit(++count === 1 ? {type: 'accepted', event_id: eventId, created_at, expires_at} : {type: 'error', code: 'duplicate_event'});
  }}); t.after(() => f.client.reset());
  assert.equal((await f.client.send(eventId, peerId, b64('opaque'))).type, 'accepted');
  await assert.rejects(f.client.send(eventId, peerId, b64('opaque')), error => error.code === 'duplicate_event');
});
test('malformed numeric IDs and unknown fields close the socket', async () => {
  for (const response of [{type: 'person', void_id: 123, auth_public_key: b64(Buffer.alloc(32))}, {type: 'person', void_id: peerId, auth_public_key: b64(Buffer.alloc(32)), trusted: 'true'}]) {
    const f = fixture({command(_frame, socket) { socket.emit(response); }});
    await assert.rejects(f.client.lookup(peerId), error => error.code === 'invalid_response'); assert.equal(f.closed, 1);
  }
});

function controllerFixture(overrides = {}) {
  const f = fixture(); const commits = []; const operations = [];
  const native = {
    signAuthChallenge: async message => b64(sign(null, Buffer.from(message, 'base64'), overrides.keys?.privateKey)),
    getState: async () => ({sessions: [], outbox: [], blockedVoidIds: []}),
    openConversation: async peer => ({id: eventId, kind: 'direct', title: peer.void_id, peerVoidId: peer.void_id, mlsGroupId: null, createdAt: 1, updatedAt: 1, unreadCount: 0, members: [], lastMessage: null}),
    resumePending: async () => {}, blockPeer: async () => {}, queueMessage: async () => {},
    completeOperation: async (...args) => { operations.push(args); }, failOperation: async () => {}, retryOperation: async () => {},
    applyOnline: async () => {}, applyEvent: async event => ({event_id: event.event_id, sender_void_id: event.from_void_id, changed: true}),
    ...overrides.driver,
  };
  const {MessagingController} = modules()('controller');
  // Delegate authentication to the fixture's actual synthetic Ed25519 signer.
  let socket;
  const keys = generateKeyPairSync('ed25519');
  const identity = {...f.identity, signaturePublicKeyBase64: keys.publicKey.export({type: 'spki', format: 'der'}).subarray(-32).toString('base64')};
  native.signAuthChallenge = async message => sign(null, Buffer.from(message, 'base64'), keys.privateKey).toString('base64');
  const controller = new MessagingController({apiUrl: base, identity: () => identity, driver: native, committed: scope => commits.push(scope),
    socketFactory: () => {
      socket = {onopen: null, onmessage: null, onerror: null, onclose: null, close() {},
        emit(frame) { socket.onmessage?.({data: JSON.stringify(frame)}); },
        send(text) {
          const frame = JSON.parse(text); operations.push(frame);
          if (frame.type === 'auth_begin') {
            const id = randomBytes(32).toString('base64url'); const expires_at = new Date(Date.now() + 10000).toISOString();
            socket.emit({type: 'auth_challenge', challenge_id: id, expires_at, message: b64(`VOID-NOTHINGNESS:authenticate:v2\n${id}\n${identity.accountVoidId}\n${identity.signaturePublicKeyBase64}\n${expires_at}\n${b64(base)}\n${randomBytes(32).toString('base64url')}\n`)});
          } else if (frame.type === 'auth_proof') { socket.emit({type: 'auth_ok', void_id: identity.accountVoidId}); }
          else if (frame.type === 'lookup') { socket.emit({type: 'person', void_id: frame.void_id, auth_public_key: identity.signaturePublicKeyBase64}); }
          else if (frame.type === 'ack') { socket.emit({type: 'ack_ok', sender_void_id: frame.sender_void_id, event_id: frame.event_id}); }
          else { overrides.command?.(frame, socket); }
        },
      }; queueMicrotask(() => socket.onopen?.()); return socket;
    },
  });
  return {controller, operations, commits, emit: frame => socket.emit(frame), disconnect: () => socket.onclose?.(), identity};
}
const incoming = () => ({type: 'event', event_id: eventId, from_void_id: peerId, payload_base64: b64('opaque'), created_at: new Date().toISOString(), expires_at: new Date(Date.now() + 100000).toISOString()});
test('recipient ACK follows native processing and committed visible history', async t => {
  let release; let applying = false;
  const f = controllerFixture({driver: {applyEvent: event => { applying = true; return new Promise(resolve => { release = () => resolve({event_id: event.event_id, sender_void_id: event.from_void_id, changed: true}); }); }}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  f.emit(incoming()); await until(() => applying); assert.equal(f.operations.some(op => op.type === 'ack'), false);
  const committed = f.commits.length; release(); await until(() => f.operations.some(op => op.type === 'ack'));
  assert.ok(f.commits.length > committed);
});
test('failed native processing never sends an ACK', async t => {
  let failed = false;
  const f = controllerFixture({driver: {applyEvent: async () => { failed = true; throw {code: 'MLS_INVALID_MESSAGE'}; }}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  f.emit(incoming()); await until(() => failed); await tick();
  assert.equal(f.operations.some(op => op.type === 'ack'), false); assert.match(f.controller.getSnapshot().error, /could not be verified/);
});
test('a replay already committed by native dedup is ACKed without a new visible write', async t => {
  const f = controllerFixture({driver: {applyEvent: async event => ({event_id: event.event_id, sender_void_id: event.from_void_id, changed: false})}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online'); await tick();
  const committed = f.commits.length; f.emit(incoming()); await until(() => f.operations.some(op => op.type === 'ack'));
  assert.equal(f.commits.length, committed);
});
test('restart resumes durable native work without a conversation-acceptance action', async t => {
  let resumed = 0;
  const f = controllerFixture({driver: {resumePending: async () => { resumed++; }}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  assert.equal(resumed, 1);
});
test('offline refresh publishes journal recovery before waiting for authentication', async t => {
  let recovered = false;
  const f = controllerFixture({driver: {getState: async () => { recovered = true; return {sessions: [], outbox: [], blockedVoidIds: []}; }}});
  t.after(() => f.controller.stop()); f.controller.start();
  await until(() => recovered && f.commits.length > 0);
  assert.equal(f.commits[0], f.identity.localDeviceId);
});
test('stop while native decryption is pending cannot ACK from the old lifecycle', async t => {
  let release; let processing = false;
  const f = controllerFixture({driver: {applyEvent: event => { processing = true; return new Promise(resolve => { release = () => resolve({event_id: event.event_id, sender_void_id: event.from_void_id, changed: true}); }); }}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  f.emit(incoming()); await until(() => processing); f.controller.stop(); release(); await tick();
  assert.equal(f.operations.some(op => op.type === 'ack'), false);
});
test('transport retry reuses saved ciphertext and records sent only on accepted', async t => {
  let outbox = []; let encrypted = 0; let accepted = 0; let attempts = 0;
  const f = controllerFixture({driver: {
    getState: async () => ({sessions: [], outbox, blockedVoidIds: []}),
    queueMessage: async () => { encrypted++; outbox = [{id: eventId, kind: 'message', peerVoidId: peerId, payloadBase64: b64('saved MLS ciphertext'), status: 'pending'}]; },
    completeOperation: async (_id, response) => { assert.equal(response.type, 'accepted'); accepted++; outbox = []; },
  }, command(frame, socket) {
    if (frame.type === 'send') { attempts++; socket.emit(attempts === 1 ? {type: 'error', code: 'unavailable'} : {type: 'accepted', event_id: eventId, created_at: new Date().toISOString(), expires_at: new Date(Date.now() + 100000).toISOString()}); }
  }});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  await f.controller.sendMessage(eventId, 'Plaintext stays native'); await until(() => attempts === 1); await tick();
  assert.equal(accepted, 0); assert.equal(encrypted, 1);
  await f.controller.refresh(); await until(() => accepted === 1);
  assert.equal(encrypted, 1); assert.equal(attempts, 2);
  const sends = f.operations.filter(op => op.type === 'send'); assert.deepEqual(sends[0], sends[1]);
});
test('duplicate queued event never becomes a sent receipt', async t => {
  let outbox = [{id: eventId, kind: 'message', peerVoidId: peerId, payloadBase64: b64('saved MLS ciphertext'), status: 'pending'}];
  let completed = 0; let failure;
  const f = controllerFixture({driver: {
    getState: async () => ({sessions: [], outbox, blockedVoidIds: []}),
    completeOperation: async () => { completed++; },
    failOperation: async (_id, code) => { failure = code; outbox = [{...outbox[0], status: 'failed', errorCode: code}]; },
  }, command(frame, socket) { if (frame.type === 'send') { socket.emit({type: 'error', code: 'duplicate_event'}); } }});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => failure !== undefined);
  assert.equal(failure, 'duplicate_event'); assert.equal(completed, 0);
});
test('opening a local thread while disconnected returns a composer without sending an invitation', async t => {
  const f = controllerFixture();
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  f.disconnect(); assert.equal(f.controller.getSnapshot().status, 'offline');
  const before = f.operations.length;
  const conversation = await f.controller.openConversation({void_id: peerId, auth_public_key: b64(Buffer.alloc(32))});
  assert.equal(conversation.id, eventId); assert.equal(conversation.mlsGroupId, null);
  assert.equal(f.operations.length, before);
});
test('first text is saved without an active group or a recipient acceptance action', async t => {
  const saved = [];
  const f = controllerFixture({driver: {queueMessage: async (id, body) => saved.push({id, body})}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  f.disconnect();
  await f.controller.sendMessage(eventId, 'Hello while the recipient is offline');
  assert.deepEqual(saved, [{id: eventId, body: 'Hello while the recipient is offline'}]);
  assert.equal(f.operations.some(op => op.type === 'relay_online' || op.type === 'send'), false);
});
test('queued setup resolves the registered sender key before native processing and ACK', async t => {
  let binding;
  const f = controllerFixture({driver: {applyEvent: async (event, peer) => {
    binding = peer; return {sender_void_id: event.from_void_id, event_id: event.event_id, changed: true};
  }}});
  t.after(() => f.controller.stop()); f.controller.start(); await until(() => f.controller.getSnapshot().status === 'online');
  f.emit(incoming()); await until(() => f.operations.some(op => op.type === 'ack'));
  assert.equal(binding.void_id, peerId); assert.equal(binding.auth_public_key, f.identity.signaturePublicKeyBase64);
  const lookup = f.operations.findIndex(op => op.type === 'lookup'); const ack = f.operations.findIndex(op => op.type === 'ack');
  assert.ok(lookup >= 0 && lookup < ack);
});
test('native verification diagnostics retain a safe category without exposing error text', () => {
  const {safeMessagingError} = modules()('errors');
  const error = safeMessagingError({code: 'MLS_INVALID_MESSAGE', message: 'private-key=never expose this'});
  assert.equal(error.code, 'secure_state'); assert.equal(error.diagnosticCode, 'MLS_INVALID_MESSAGE');
  assert.equal(error.message.includes('private-key'), false);
  assert.equal(safeMessagingError({code: 'PEER_KEY_CHANGED'}).code, 'identity_changed');
});
