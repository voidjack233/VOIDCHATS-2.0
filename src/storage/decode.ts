import {StorageError} from './bridge';
import type {ContactRecord, ConversationCursor, ConversationMember, ConversationPage, ConversationRecord, LocalIdentity, MessagePage, MessageRecord, MessageRequestRecord, NativeStorageSnapshot} from './types';

type RecordValue = Record<string, unknown>;
function object(value: unknown, fields?: readonly string[]): RecordValue {
  if (!value || typeof value !== 'object' || Array.isArray(value)) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  if (Object.keys(value).some(key => /private|secret|snapshot|passphrase|databasekey/i.test(key))) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  if (fields && Object.keys(value).some(key => !fields.includes(key))) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return value as RecordValue;
}
function text(value: unknown): string {
  if (typeof value !== 'string' || value.includes('\0')) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return value;
}
function nullableText(value: unknown): string | null { return value === null ? null : text(value); }
function number(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return value;
}
function nullableNumber(value: unknown): number | null { return value === null ? null : number(value); }
function enumeration<T extends string>(value: unknown, values: readonly T[]): T {
  if (typeof value !== 'string' || !values.includes(value as T)) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return value as T;
}
export function decodeList<T>(value: unknown, decode: (item: unknown) => T): T[] {
  if (!Array.isArray(value)) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return value.map(decode);
}
export function decodeNullable<T>(value: unknown, decode: (item: unknown) => T): T | null {
  return value === null ? null : decode(value);
}
export function decodeIdentity(value: unknown): LocalIdentity {
  const row = object(value);
  return {
    localDeviceId: text(row.localDeviceId), clientHandle: text(row.clientHandle),
    credentialIdentityBase64: text(row.credentialIdentityBase64),
    signaturePublicKeyBase64: text(row.signaturePublicKeyBase64),
    accountVoidId: nullableText(row.accountVoidId), createdAt: number(row.createdAt),
  };
}
export function decodeSnapshot(value: unknown): NativeStorageSnapshot {
  const row = object(value);
  return {
    status: enumeration(row.status, ['not_initialized', 'locked', 'open', 'error']),
    schemaVersion: nullableNumber(row.schemaVersion),
    identity: decodeNullable(row.identity, decodeIdentity),
    errorCode: nullableText(row.errorCode),
  };
}
function decodeMember(value: unknown): ConversationMember {
  const row = object(value);
  return {voidId: text(row.voidId), displayName: text(row.displayName),
    role: enumeration(row.role, ['member', 'admin']), joinedAt: number(row.joinedAt), leftAt: nullableNumber(row.leftAt)};
}
export function decodeConversation(value: unknown): ConversationRecord {
  const row = object(value, ['id', 'kind', 'title', 'peerVoidId', 'mlsGroupId', 'createdAt', 'updatedAt', 'unreadCount', 'members', 'lastMessage']);
  return {
    id: text(row.id), kind: enumeration(row.kind, ['direct', 'group']), title: text(row.title),
    peerVoidId: nullableText(row.peerVoidId), mlsGroupId: nullableText(row.mlsGroupId),
    createdAt: number(row.createdAt), updatedAt: number(row.updatedAt), unreadCount: number(row.unreadCount),
    members: decodeList(row.members, decodeMember),
    lastMessage: decodeNullable(row.lastMessage, entry => {
      const preview = object(entry);
      return {id: text(preview.id), body: nullableText(preview.body), createdAt: number(preview.createdAt), deletedAt: nullableNumber(preview.deletedAt)};
    }),
  };
}

// SQLite BINARY UTF-8 ordering follows Unicode scalar order, unlike JS UTF-16.
function compareIds(left: string, right: string): number {
  const a = Array.from(left, character => character.codePointAt(0)!);
  const b = Array.from(right, character => character.codePointAt(0)!);
  for (let index = 0; index < Math.min(a.length, b.length); index++) {
    if (a[index] !== b[index]) { return a[index] - b[index]; }
  }
  return a.length - b.length;
}

function conversationCursor(value: unknown): ConversationCursor {
  const row = object(value, ['updatedAt', 'id']);
  const id = text(row.id);
  if (!id.trim() || id.length > 512) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return {updatedAt: number(row.updatedAt), id};
}

function compareConversation(left: ConversationCursor, right: ConversationCursor): number {
  return left.updatedAt === right.updatedAt ? compareIds(left.id, right.id) : left.updatedAt - right.updatedAt;
}

export function decodeConversationPage(value: unknown, before: ConversationCursor | null = null, limit = 100): ConversationPage {
  const row = object(value, ['items', 'nextCursor']);
  const items = decodeList(row.items, decodeConversation);
  if (items.length > limit || items.length > 100 || new Set(items.map(item => item.id)).size !== items.length) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  for (let index = 0; index < items.length; index++) {
    const item = conversationCursor({id: items[index].id, updatedAt: items[index].updatedAt});
    if ((index === 0 && before !== null && compareConversation(item, before) >= 0) ||
        (index > 0 && compareConversation(item, items[index - 1]) >= 0)) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  }
  const nextCursor = decodeNullable(row.nextCursor, conversationCursor);
  if (nextCursor) {
    const last = items[items.length - 1];
    if (!last || last.id !== nextCursor.id || last.updatedAt !== nextCursor.updatedAt) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  }
  return {items, nextCursor};
}
export function decodeMessage(value: unknown): MessageRecord {
  const row = object(value);
  const result: MessageRecord = {
    id: text(row.id), conversationId: text(row.conversationId), senderVoidId: text(row.senderVoidId),
    body: nullableText(row.body), direction: enumeration(row.direction, ['incoming', 'outgoing']),
    deliveryState: enumeration(row.deliveryState, ['received', 'pending', 'sent', 'delivered', 'read', 'failed']),
    createdAt: number(row.createdAt), editedAt: nullableNumber(row.editedAt), deletedAt: nullableNumber(row.deletedAt), readAt: nullableNumber(row.readAt),
    reactions: decodeList(row.reactions, entry => {
      const reaction = object(entry);
      return {reactorVoidId: text(reaction.reactorVoidId), emoji: text(reaction.emoji), createdAt: number(reaction.createdAt)};
    }),
    receipts: decodeList(row.receipts, entry => {
      const receipt = object(entry);
      return {recipientVoidId: text(receipt.recipientVoidId), state: enumeration(receipt.state, ['delivered', 'read']), recordedAt: number(receipt.recordedAt)};
    }),
  };
  if ((result.deletedAt === null) !== (result.body !== null)) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  return result;
}
export function decodePage(value: unknown): MessagePage {
  const row = object(value);
  return {items: decodeList(row.items, decodeMessage), nextCursor: decodeNullable(row.nextCursor, entry => {
    const cursor = object(entry);
    return {createdAt: number(cursor.createdAt), id: text(cursor.id)};
  })};
}
export function decodeContact(value: unknown): ContactRecord {
  const row = object(value);
  return {id: text(row.id), voidId: text(row.voidId), displayName: text(row.displayName),
    identityPublicKey: text(row.identityPublicKey), trustState: enumeration(row.trustState, ['unverified', 'changed']),
    createdAt: number(row.createdAt), updatedAt: number(row.updatedAt)};
}
export function decodeRequest(value: unknown): MessageRequestRecord {
  const row = object(value);
  return {id: text(row.id), conversationId: nullableText(row.conversationId), fromVoidId: text(row.fromVoidId),
    displayName: text(row.displayName), identityPublicKey: text(row.identityPublicKey), preview: text(row.preview),
    status: enumeration(row.status, ['pending', 'accepted', 'declined', 'blocked']),
    createdAt: number(row.createdAt), updatedAt: number(row.updatedAt)};
}

