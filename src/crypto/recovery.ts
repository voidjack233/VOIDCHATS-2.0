import type {MlsDecryptionCursor, MlsStoredDecryption, MlsStoredDecryptionPage} from './types';

function invalid(): never { throw new Error('The stored MLS application data response is invalid.'); }
function record(value: unknown, fields: readonly string[]): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) { return invalid(); }
  if (Object.keys(value).some(key => !fields.includes(key))) { return invalid(); }
  return value as Record<string, unknown>;
}
function timestamp(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) { return invalid(); }
  return value;
}
function identifier(value: unknown): string {
  if (typeof value !== 'string' || !value.trim() || value.length > 512 || value.includes('\0')) { return invalid(); }
  return value;
}
function base64(value: unknown): string {
  if (typeof value !== 'string' || value.length > 1_398_104 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) { return invalid(); }
  return value;
}
export function validateDecryptionCursor(value: MlsDecryptionCursor): MlsDecryptionCursor {
  const cursor = record(value, ['createdAt', 'id']);
  return {createdAt: timestamp(cursor.createdAt), id: identifier(cursor.id)};
}

/** Accepted application payloads only; no serialized private MLS records. */
export function decodeStoredDecryptions(input: unknown): MlsStoredDecryptionPage {
  if (typeof input !== 'string' || input.length > 4 * 1024 * 1024) { return invalid(); }
  let value: unknown;
  try { value = JSON.parse(input) as unknown; } catch { return invalid(); }
  const page = record(value, ['items', 'nextCursor']);
  if (!Array.isArray(page.items) || page.items.length > 100) { return invalid(); }
  const items = page.items.map((entry: unknown): MlsStoredDecryption => {
    const message = record(entry, ['eventId', 'groupIdBase64', 'plaintextBase64', 'senderIdentityBase64', 'senderSignaturePublicKeyBase64', 'epoch', 'createdAt']);
    const epoch = message.epoch;
    if (typeof epoch !== 'string' || !/^(0|[1-9][0-9]{0,19})$/.test(epoch) || (epoch.length === 20 && epoch > '18446744073709551615')) { return invalid(); }
    return {
      eventId: identifier(message.eventId), groupIdBase64: base64(message.groupIdBase64),
      plaintextBase64: base64(message.plaintextBase64), senderIdentityBase64: base64(message.senderIdentityBase64),
      senderSignaturePublicKeyBase64: base64(message.senderSignaturePublicKeyBase64), epoch,
      createdAt: timestamp(message.createdAt),
    };
  });
  if (new Set(items.map(item => item.eventId)).size !== items.length) { return invalid(); }
  for (let index = 1; index < items.length; index++) {
    const previous = items[index - 1];
    const current = items[index];
    if (current.createdAt > previous.createdAt || (current.createdAt === previous.createdAt && current.eventId >= previous.eventId)) { return invalid(); }
  }
  const nextCursor = page.nextCursor === null ? null : validateDecryptionCursor(page.nextCursor as MlsDecryptionCursor);
  if (nextCursor) {
    const last = items[items.length - 1];
    if (!last || last.eventId !== nextCursor.id || last.createdAt !== nextCursor.createdAt) { return invalid(); }
  }
  return {items, nextCursor};
}
