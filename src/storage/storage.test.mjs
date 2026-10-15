import assert from 'node:assert/strict';
import {readFileSync, unlinkSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {randomUUID} from 'node:crypto';
import {DatabaseSync} from 'node:sqlite';
import test from 'node:test';
import {runInNewContext} from 'node:vm';
import ts from 'typescript';

// Host-only SQLite tests use isolated synthetic fixtures, never app/user data.
const migration = readFileSync(new URL('../../android/app/src/main/assets/storage/001_initial.sql', import.meta.url), 'utf8');
function database(t, path = ':memory:') {
  const db = new DatabaseSync(path);
  db.exec('PRAGMA foreign_keys=ON');
  db.exec(migration);
  t.after(() => db.close());
  return db;
}
function direct(db, id = 'conversation', peer = 'peer', at = 1000) {
  db.prepare("INSERT INTO conversations(id,kind,title,peer_void_id,created_at,updated_at) VALUES(?,'direct','Synthetic conversation',?,?,?)").run(id, peer, at, at);
}
function incoming(db, id, at = 2000, conversation = 'conversation') {
  db.prepare("INSERT INTO messages(id,conversation_id,sender_void_id,body,direction,delivery_state,created_at) VALUES(?,?,?,'Synthetic message','incoming','received',?)").run(id, conversation, 'peer', at);
}
function modules(globals = {}) {
  const cache = new Map();
  const load = name => {
    if (cache.has(name)) { return cache.get(name).exports; }
    const module = {exports: {}};
    cache.set(name, module);
    const source = readFileSync(new URL('./' + name + '.ts', import.meta.url), 'utf8');
    const {outputText} = ts.transpileModule(source, {compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}});
    runInNewContext(outputText, {module, exports: module.exports, Error, Date, Set, Map, Object, JSON,
      require: path => { assert.ok(path.startsWith('./')); return load(path.slice(2)); }, ...globals});
    return module.exports;
  };
  return load;
}
const identity = {localDeviceId: 'synthetic-device', clientHandle: 'synthetic-client', credentialIdentityBase64: 'YQ==', signaturePublicKeyBase64: 'Yg==', accountVoidId: null, createdAt: 1000};
const snapshot = (status = 'open') => ({status, identity: status === 'open' ? identity : null, schemaVersion: status === 'not_initialized' ? null : 1, errorCode: null});
const contact = {id: 'contact', voidId: 'peer', displayName: 'Synthetic peer', identityPublicKey: 'public-fixture', trustState: 'unverified', createdAt: 1000, updatedAt: 1000};
function native(overrides = {}) {
  return {getStatus: async () => JSON.stringify(snapshot()), open: async () => JSON.stringify(snapshot()),
    initializeDevelopment: async () => JSON.stringify(snapshot()), close: async () => JSON.stringify(snapshot('locked')),
    removeLocalData: async () => JSON.stringify(snapshot('not_initialized')), getLocalIdentity: async () => JSON.stringify(identity),
    execute: async () => 'null', ...overrides};
}
function driver(load, module, development = true, platform = 'android') {
  return load('bridge').createStorageDriver(() => module, platform, development, load('decode').decodeSnapshot);
}

test('migration creates public models and isolated native state without fixture rows', t => {
  const db = database(t);
  for (const table of ['contacts', 'conversations', 'conversation_members', 'messages', 'message_reactions', 'message_receipts', 'message_requests', 'local_identity', 'native_mls_state', 'crypto_events', 'storage_migrations']) {
    assert.equal(db.prepare('SELECT count(*) AS count FROM ' + table).get().count, 0);
  }
  assert.throws(() => db.exec(migration));
  assert.throws(() => db.prepare("INSERT INTO app_settings VALUES('identity_private_key','forbidden',0)").run());
});

test('direct/group metadata, duplicate peers, and foreign keys are enforced', t => {
  const db = database(t);
  direct(db);
  assert.throws(() => direct(db, 'duplicate', 'peer'));
  assert.throws(() => db.exec("INSERT INTO conversations VALUES('bad','direct','Title',NULL,NULL,0,0,NULL,NULL)"));
  db.exec("INSERT INTO conversations VALUES('group','group','Synthetic group',NULL,'group-public-id',1000,1000,NULL,NULL)");
  db.exec("INSERT INTO conversation_members VALUES('group','member','Synthetic member','admin',1000,NULL)");
  assert.throws(() => incoming(db, 'orphan', 2000, 'missing'));
  db.prepare('DELETE FROM conversations WHERE id=?').run('group');
  assert.equal(db.prepare('SELECT count(*) AS count FROM conversation_members').get().count, 0);
});

test('stable cursor pagination handles equal timestamps and new inserts without duplicates', t => {
  const db = database(t); direct(db);
  for (const id of ['a', 'b', 'c']) { incoming(db, id, 3000); }
  incoming(db, 'older', 2000);
  const first = db.prepare('SELECT id,created_at FROM messages WHERE conversation_id=? ORDER BY created_at DESC,id DESC LIMIT ?').all('conversation', 2);
  assert.deepEqual(first.map(row => row.id), ['c', 'b']);
  incoming(db, 'newer', 4000);
  const before = first.at(-1);
  const second = db.prepare('SELECT id FROM messages WHERE conversation_id=? AND (created_at < ? OR (created_at = ? AND id < ?)) ORDER BY created_at DESC,id DESC LIMIT ?')
    .all('conversation', before.created_at, before.created_at, before.id, 2);
  assert.deepEqual(second.map(row => row.id), ['a', 'older']);
});

test('read timestamps preserve unread late arrivals and conversation isolation', t => {
  const db = database(t); direct(db); direct(db, 'other', 'other-peer');
  incoming(db, 'visible', 3000); incoming(db, 'other-message', 1000, 'other');
  db.prepare('UPDATE messages SET read_at=? WHERE conversation_id=? AND (created_at < ? OR (created_at=? AND id<=?))').run(5000, 'conversation', 3000, 3000, 'visible');
  incoming(db, 'late', 2000);
  const unread = db.prepare("SELECT id FROM messages WHERE conversation_id=? AND direction='incoming' AND read_at IS NULL AND deleted_at IS NULL").all('conversation');
  assert.deepEqual(unread.map(row => row.id), ['late']);
  assert.equal(db.prepare('SELECT read_at FROM messages WHERE id=?').get('other-message').read_at, null);
});

test('edits retain message identity and deletion clears body/reactions without resurrection', t => {
  const db = database(t); direct(db); incoming(db, 'message');
  db.prepare('UPDATE messages SET body=?,edited_at=? WHERE id=?').run('Edited synthetic message', 3000, 'message');
  assert.equal(db.prepare('SELECT created_at FROM messages WHERE id=?').get('message').created_at, 2000);
  assert.throws(() => db.prepare('UPDATE messages SET sender_void_id=? WHERE id=?').run('different-peer', 'message'));
  db.exec("INSERT INTO message_reactions VALUES('message','peer','👍',3000)");
  assert.throws(() => db.exec("INSERT INTO message_reactions VALUES('message','peer','👍',3000)"));
  db.prepare('UPDATE messages SET body=NULL,deleted_at=? WHERE id=?').run(4000, 'message');
  assert.equal(db.prepare('SELECT body FROM messages WHERE id=?').get('message').body, null);
  assert.equal(db.prepare('SELECT count(*) AS count FROM message_reactions').get().count, 0);
  assert.throws(() => db.exec("UPDATE messages SET deleted_at=NULL,body='revived' WHERE id='message'"));
  assert.throws(() => db.exec("INSERT INTO message_reactions VALUES('message','peer','👍',5000)"));
});

test('delivery and recipient receipt states cannot regress', t => {
  const db = database(t); direct(db);
  db.exec("INSERT INTO messages(id,conversation_id,sender_void_id,body,direction,delivery_state,created_at) VALUES('out','conversation','self','Synthetic outgoing','outgoing','pending',1000)");
  assert.throws(() => db.exec("UPDATE messages SET delivery_state='read' WHERE id='out'"));
  db.exec("UPDATE messages SET delivery_state='sent' WHERE id='out'");
  db.exec("UPDATE messages SET delivery_state='delivered' WHERE id='out'");
  db.exec("UPDATE messages SET delivery_state='read' WHERE id='out'");
  assert.throws(() => db.exec("UPDATE messages SET delivery_state='sent' WHERE id='out'"));
  db.exec("INSERT INTO message_receipts VALUES('out','peer','delivered',2000)");
  db.exec("UPDATE message_receipts SET state='read',recorded_at=3000 WHERE message_id='out'");
  assert.throws(() => db.exec("UPDATE message_receipts SET state='delivered' WHERE message_id='out'"));
});

test('contacts stay unverified and changed public keys require an explicit changed marker', t => {
  const db = database(t);
  db.exec("INSERT INTO contacts VALUES('contact','peer','Synthetic peer','public-key','unverified',1000,1000)");
  assert.throws(() => db.exec("INSERT INTO contacts VALUES('duplicate','peer','Duplicate','key','unverified',1000,1000)"));
  assert.throws(() => db.exec("UPDATE contacts SET trust_state='verified'"));
  assert.throws(() => db.exec("UPDATE contacts SET identity_public_key='changed-key'"));
  db.exec("UPDATE contacts SET identity_public_key='changed-key',trust_state='changed',updated_at=2000");
  assert.equal(db.prepare('SELECT trust_state FROM contacts').get().trust_state, 'changed');
});

test('request transitions and native engine CAS survive a real database reopen', () => {
  const path = join(tmpdir(), 'void-storage-tests-' + randomUUID() + '.sqlite');
  let db;
  try {
    db = new DatabaseSync(path); db.exec('PRAGMA foreign_keys=ON'); db.exec(migration);
    db.exec("INSERT INTO message_requests VALUES('request',NULL,'peer','Synthetic peer','public-key','Synthetic request','pending',1000,1000)");
    db.exec("UPDATE message_requests SET status='accepted',updated_at=2000 WHERE id='request'");
    assert.throws(() => db.exec("UPDATE message_requests SET status='pending' WHERE id='request'"));
    db.prepare('INSERT INTO native_mls_state VALUES(1,1,?)').run(Buffer.from('SYNTHETIC-TEST-STATE'));
    assert.throws(() => db.prepare('INSERT INTO native_mls_state VALUES(2,1,?)').run(Buffer.from('OTHER-TEST-STATE')));
    const update = db.prepare('UPDATE native_mls_state SET revision=revision+1,snapshot=? WHERE slot=1 AND revision=?');
    assert.equal(update.run(Buffer.from('SYNTHETIC-REVISION-2'), 1).changes, 1);
    assert.equal(update.run(Buffer.from('STALE-MUST-NOT-REPLACE'), 1).changes, 0);
    db.close(); db = new DatabaseSync(path);
    assert.equal(db.prepare('SELECT status FROM message_requests WHERE id=?').get('request').status, 'accepted');
    assert.equal(db.prepare('SELECT revision FROM native_mls_state').get().revision, 2);
    assert.equal(Buffer.from(db.prepare('SELECT snapshot FROM native_mls_state').get().snapshot).toString(), 'SYNTHETIC-REVISION-2');
  } finally {
    db?.close();
    try { unlinkSync(path); } catch (error) { if (error.code !== 'ENOENT') { throw error; } }
  }
});

test('native bridge is lazy, release-gated, and excludes arbitrary SQL/secret operations', async () => {
  const load = modules(); let opens = 0; let initializes = 0; let executes = 0;
  const module = native({open: async () => { opens++; return JSON.stringify(snapshot()); },
    initializeDevelopment: async () => { initializes++; return JSON.stringify(snapshot()); }, execute: async () => { executes++; return '[]'; }});
  const storage = driver(load, module, false);
  assert.equal(opens, 0);
  await storage.open(); assert.equal(opens, 1); assert.equal(initializes, 0);
  await assert.rejects(storage.initializeDevelopment(), {code: 'DEVELOPMENT_ONLY'});
  await assert.rejects(storage.execute('native_mls_state.list', {}), {code: 'INVALID_INPUT'});
  assert.equal(executes, 0);
  await assert.rejects(driver(load, module, true, 'web').open(), {code: 'UNSUPPORTED_PLATFORM'});
  await assert.rejects(driver(load, undefined).open(), {code: 'NATIVE_MODULE_UNAVAILABLE'});
});

test('typed repositories preserve payloads and notify only after committed validated writes', async () => {
  const load = modules(); const calls = []; let mutations = 0;
  const module = native({execute: async (operation, payload) => {
    calls.push({operation, payload: JSON.parse(payload)});
    return JSON.stringify(operation === 'contacts.save' ? contact : []);
  }});
  const repositories = load('repositories').createRepositories(driver(load, module), () => mutations++);
  const input = {...contact}; delete input.trustState;
  const stored = await repositories.contacts.save(input);
  assert.equal(stored.trustState, 'unverified'); assert.equal(mutations, 1);
  await repositories.contacts.list(); assert.equal(mutations, 1);
  assert.deepEqual(calls[0].payload, input);
  assert.throws(() => repositories.contacts.save({...input, identity_private_key: 'MUST-NOT-CROSS-BRIDGE'}), {code: 'INVALID_INPUT'});
  assert.throws(() => repositories.messages.save({id:'out',conversationId:'conversation',senderVoidId:'self',body:'Synthetic',direction:'outgoing',deliveryState:'sent',createdAt:0}), {code: 'INVALID_INPUT'});
  assert.equal(calls.length, 2);
  await repositories.messages.page('conversation', {createdAt: 123, id: 'tie'}, 7).catch(() => {});
  assert.deepEqual(calls.at(-1).payload, {conversationId:'conversation',before:{createdAt:123,id:'tie'},limit:7});
});

test('native failures and secret-shaped responses never expose raw exception text', async () => {
  const load = modules();
  const module = native({execute: async () => { throw {code:'DATABASE_ERROR',message:'RAW-SECRET-MARKER'}; }});
  const repositories = load('repositories').createRepositories(driver(load, module));
  await assert.rejects(repositories.contacts.list(), error => error.code === 'DATABASE_ERROR' && !error.message.includes('RAW-SECRET'));
  module.execute = async () => JSON.stringify({...contact, secretSnapshot:'RAW-SECRET-MARKER'});
  await assert.rejects(repositories.contacts.get('contact'), {code:'INVALID_NATIVE_RESPONSE'});
  module.open = async () => '{malformed';
  await assert.rejects(driver(load, module).open(), {code:'INVALID_NATIVE_RESPONSE'});
});

test('controller resumes existing identity, serializes close, gates reads, and recovers after key failure', async () => {
  const load = modules(); let initializeCount = 0; const order = [];
  const module = native({open: async () => {order.push('open'); return JSON.stringify(snapshot());},
    close: async () => {order.push('close'); return JSON.stringify(snapshot('locked'));},
    initializeDevelopment: async () => {initializeCount++; return JSON.stringify(snapshot());}});
  const controller = new (load('controller').StorageController)(driver(load, module));
  await Promise.all([controller.openExisting(), controller.closeStore()]);
  assert.deepEqual(order, ['open','close']); assert.equal(initializeCount, 0);
  assert.equal(controller.getSnapshot().status, 'locked');
  await assert.rejects(controller.repositories.contacts.list(), {code:'LOCKED'});
  module.open = async () => {throw {code:'KEY_UNAVAILABLE',message:'RAW-KEY-MARKER'};};
  await assert.rejects(controller.openExisting(), {code:'KEY_UNAVAILABLE'});
  assert.equal(controller.getSnapshot().identity, null);
  assert.ok(!controller.getSnapshot().message.includes('RAW-KEY'));
  module.open = async () => JSON.stringify(snapshot());
  await controller.openExisting();
  assert.equal(controller.getSnapshot().identity.clientHandle, identity.clientHandle);
  assert.equal(controller.getSnapshot().identity.accountVoidId, null);
  assert.equal(initializeCount, 0);
  const previous = controller.getSnapshot().revision;
  module.execute = async () => JSON.stringify(contact);
  const input = {...contact}; delete input.trustState;
  controller.subscribe(() => {throw new Error('view failure');});
  await controller.repositories.contacts.save(input);
  assert.equal(controller.getSnapshot().revision, previous + 1);
  await controller.removeLocalData();
  assert.equal(controller.getSnapshot().status, 'not_initialized');
  assert.equal(controller.getSnapshot().identity, null);
});

test('cached records stay hidden across identity removal/recreation and conversation changes', () => {
  const load = modules();
  const {isCurrentScope} = load('scope');
  const cached = {localDeviceId:'old-device',collection:'messages:old-conversation'};
  assert.equal(isCurrentScope(cached, 'old-device', 'messages:old-conversation'), true);
  assert.equal(isCurrentScope(cached, null, 'messages:old-conversation'), false);
  assert.equal(isCurrentScope(cached, 'new-device', 'messages:old-conversation'), false);
  assert.equal(isCurrentScope(cached, 'old-device', 'messages:new-conversation'), false);
  assert.equal(isCurrentScope(null, 'new-device', 'messages:new-conversation'), false);
});

test('native journal retains authenticated sender, exact u64 epoch and checkpoint for later ingestion', t => {
  const db = database(t);
  const insert = db.prepare('INSERT INTO crypto_events(id,operation,outcome,sender_identity,sender_signature_public_key,epoch,checkpoint_revision,created_at) VALUES(?,?,?,?,?,?,?,?)');
  insert.run('event','decrypt','ok',Buffer.from('SYNTHETIC-PUBLIC-IDENTITY'),Buffer.from('SYNTHETIC-PUBLIC-KEY'),'18446744073709551615',7,1000);
  const event = db.prepare('SELECT epoch,checkpoint_revision,sender_signature_public_key FROM crypto_events WHERE id=?').get('event');
  assert.equal(event.epoch,'18446744073709551615'); assert.equal(event.checkpoint_revision,7);
  assert.equal(Buffer.from(event.sender_signature_public_key).toString(),'SYNTHETIC-PUBLIC-KEY');
  assert.throws(() => insert.run('overflow','decrypt','ok',null,null,'18446744073709551616',7,1000));
  assert.throws(() => insert.run('invalid','decrypt','ok',null,null,'1.5',7,1000));
  assert.throws(() => insert.run('revision-zero','decrypt','ok',null,null,'1',0,1000));
});

test('conversation cursor pages remain stable across activity ties and newer inserts', t => {
  const db = database(t);
  for (const id of ['a', 'b', 'c']) { direct(db, id, 'peer-' + id, 3000); }
  direct(db, 'older', 'peer-older', 2000);
  const first = db.prepare('SELECT id,updated_at FROM conversations ORDER BY updated_at DESC,id DESC LIMIT ?').all(2);
  assert.deepEqual(first.map(row => row.id), ['c', 'b']);
  direct(db, 'newer', 'peer-newer', 4000);
  const before = first.at(-1);
  const second = db.prepare('SELECT id FROM conversations WHERE updated_at < ? OR (updated_at = ? AND id < ?) ORDER BY updated_at DESC,id DESC LIMIT ?')
    .all(before.updated_at, before.updated_at, before.id, 2);
  assert.deepEqual(second.map(row => row.id), ['a', 'older']);
  assert.equal(new Set([...first, ...second].map(row => row.id)).size, 4);
});

const conversation = (id, updatedAt = 2000) => ({id, kind:'direct', title:'Synthetic conversation', peerVoidId:'peer-' + id, mlsGroupId:null, createdAt:1000, updatedAt, unreadCount:0, lastMessage:null, members:[]});

test('conversation page decoder rejects malformed, unordered, duplicated and out-of-range pages', () => {
  const load = modules(); const {decodeConversationPage} = load('decode');
  const page = {items:[conversation('c'),conversation('b')],nextCursor:{updatedAt:2000,id:'b'}};
  assert.equal(decodeConversationPage(page, null, 2).nextCursor.id, 'b');
  for (const invalid of [
    {...page,extra:'not-a-page-field'},
    {...page,items:[conversation('b'),conversation('c')]},
    {...page,items:[conversation('b'),conversation('b')]},
    {...page,nextCursor:{updatedAt:2000,id:'wrong'}},
    {...page,nextCursor:{updatedAt:2000,id:'b',extra:true}},
    {...page,items:[]},
    {...page,items:[{...conversation('c'),updatedAt:-1}]},
    {...page,items:[{...conversation('c'),secretSnapshot:'FORBIDDEN'}]},
  ]) { assert.throws(() => decodeConversationPage(invalid), {code:'INVALID_NATIVE_RESPONSE'}); }
  assert.throws(() => decodeConversationPage(page, null, 1), {code:'INVALID_NATIVE_RESPONSE'});
  assert.throws(() => decodeConversationPage(page, {updatedAt:2000,id:'b'}), {code:'INVALID_NATIVE_RESPONSE'});
  const unicode = {items:[conversation('𐀀'),conversation('')],nextCursor:null};
  assert.equal(decodeConversationPage(unicode).items[0].id, '𐀀');
});

test('typed conversation paging uses bounded native operation without mutation notifications', async () => {
  const load = modules(); const calls = []; let mutations = 0;
  const module = native({execute: async (operation, payloadJSON) => {
    calls.push({operation,payload:JSON.parse(payloadJSON)});
    return JSON.stringify({items:[conversation('b')],nextCursor:null});
  }});
  const repositories = load('repositories').createRepositories(driver(load,module), () => mutations++);
  const page = await repositories.conversations.page({updatedAt:3000,id:'prior'}, 2);
  assert.equal(page.items[0].id, 'b'); assert.equal(mutations,0);
  assert.deepEqual(calls[0],{operation:'conversations.page',payload:{before:{updatedAt:3000,id:'prior'},limit:2}});
  for (const limit of [0,-1,101,1.5,Number.NaN]) {
    assert.throws(() => repositories.conversations.page(null,limit),{code:'INVALID_INPUT'});
  }
  assert.throws(() => repositories.conversations.page({updatedAt:Number.MAX_SAFE_INTEGER + 1,id:'id'}),{code:'INVALID_INPUT'});
  assert.throws(() => repositories.conversations.page({updatedAt:0,id:''}),{code:'INVALID_INPUT'});
  assert.equal(calls.length,1);
});

test('corruption and unavailable encryption preserve distinct sanitized fail-closed states', async () => {
  for (const code of ['CORRUPT','ENCRYPTION_UNAVAILABLE']) {
    const load = modules(); const module = native();
    const controller = new (load('controller').StorageController)(driver(load,module));
    await controller.openExisting();
    module.execute = async () => { throw {code,message:'RAW-PRIVATE-MARKER'}; };
    await assert.rejects(controller.repositories.contacts.list(),{code});
    assert.equal(controller.getSnapshot().status,'error');
    assert.equal(controller.getSnapshot().errorCode,code);
    assert.equal(controller.getSnapshot().identity,null);
    assert.ok(!controller.getSnapshot().message.includes('RAW-PRIVATE'));
    await assert.rejects(controller.repositories.conversations.page(),{code:'LOCKED'});
    module.open = async () => { throw {code,message:'RAW-PRIVATE-MARKER'}; };
    await assert.rejects(controller.openExisting(),{code});
    assert.equal(controller.getSnapshot().errorCode,code);
  }
});

