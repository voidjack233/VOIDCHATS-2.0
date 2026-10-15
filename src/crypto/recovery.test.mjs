import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

const source = readFileSync(new URL('./recovery.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, {compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022}}).outputText;
const exports = {};
vm.runInNewContext(compiled, {exports});
const {decodeStoredDecryptions, validateDecryptionCursor} = exports;
const message = {
  eventId: 'event-b', groupIdBase64: 'Z3JvdXA=', plaintextBase64: '',
  senderIdentityBase64: 'c2VuZGVy', senderSignaturePublicKeyBase64: 'cHVibGljLWtleQ==',
  epoch: '18446744073709551615', createdAt: 100,
};

test('committed application data keeps exact epochs and empty plaintext', () => {
  const result = decodeStoredDecryptions(JSON.stringify({items: [message], nextCursor: {createdAt: 100, id: 'event-b'}}));
  assert.equal(result.items[0].epoch, '18446744073709551615');
  assert.equal(result.items[0].plaintextBase64, '');
  assert.equal(result.nextCursor.id, 'event-b');
});
test('corrupt, private, duplicate and unordered recovery rows fail closed', () => {
  for (const items of [
    [{...message, snapshot: 'SYNTHETIC-SECRET'}],
    [{...message, epoch: '18446744073709551616'}],
    [{...message, epoch: '01'}],
    [{...message, plaintextBase64: 'not base64!'}],
    [{...message, senderIdentityBase64: null}],
    [message, message],
    [{...message, eventId: 'event-a'}, message],
  ]) {
    assert.throws(() => decodeStoredDecryptions(JSON.stringify({items, nextCursor: null})), error => {
      assert.equal(error.message, 'The stored MLS application data response is invalid.');
      assert.equal(error.message.includes('SYNTHETIC-SECRET'), false);
      return true;
    });
  }
});
test('recovery cursors bind the last returned row and reject unsafe timestamps', () => {
  assert.throws(() => decodeStoredDecryptions(JSON.stringify({items: [message], nextCursor: {createdAt: 100, id: 'other'}})));
  assert.throws(() => validateDecryptionCursor({createdAt: Number.MAX_SAFE_INTEGER + 1, id: 'event-b'}));
  assert.throws(() => validateDecryptionCursor({createdAt: 100, id: 'event-b', secret: 'unused'}));
});
test('oversized or malformed bridge pages reject before use', () => {
  assert.throws(() => decodeStoredDecryptions('{bad json'));
  assert.throws(() => decodeStoredDecryptions(' '.repeat(4 * 1024 * 1024 + 1)));
  assert.throws(() => decodeStoredDecryptions(JSON.stringify({items: [], nextCursor: null, snapshot: 'unused'})));
});
