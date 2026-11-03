import {isTimestamp, isVoidId} from '../account/validation';
import {MessagingError} from './errors';
import type {MessagingSnapshot, Person, PreKeyErrorCode, ServerFrame} from './types';

const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const invalid = (): never => { throw new MessagingError('invalid_response'); };
export function record(value: unknown, fields?: readonly string[]): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) { return invalid(); }
  const row = value as Record<string, unknown>;
  if (Object.keys(row).some(key => /private|secret|passphrase|databasekey/i.test(key) || (fields && !fields.includes(key)))) { return invalid(); }
  return row;
}
export function string(value: unknown): string { return typeof value === 'string' && !value.includes('\0') ? value : invalid(); }
export function uuid(value: unknown): string {
  const id = string(value);
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id) && id !== '00000000-0000-0000-0000-000000000000' ? id : invalid();
}
export function exchangeId(value: unknown): string { const id = string(value); return /^[A-Za-z0-9._:-]{1,64}$/.test(id) ? id : invalid(); }
export function voidId(value: unknown): string { return isVoidId(value) ? value : invalid(); }
export function timestamp(value: unknown): string { return isTimestamp(value) ? value : invalid(); }
function preKeyCount(value: unknown): string {
  const count = string(value);
  return /^(0|[1-9][0-9]{0,3})$/.test(count) && Number(count) <= 4096 ? count : invalid();
}
export function base64(value: unknown, maximum = 1048576, exact?: number): string {
  const encoded = string(value);
  if (!encoded || encoded.length > Math.ceil(maximum / 3) * 4 || encoded.length % 4 || !/^[A-Za-z0-9+/]+={0,2}$/.test(encoded)) { return invalid(); }
  const padding = encoded.endsWith('==') ? 2 : encoded.endsWith('=') ? 1 : 0;
  const size = encoded.length / 4 * 3 - padding;
  if (size < 1 || size > maximum || (exact !== undefined && size !== exact)) { return invalid(); }
  const last = alphabet.indexOf(encoded[encoded.length - padding - 1]);
  if ((padding === 2 && last % 16 !== 0) || (padding === 1 && last % 4 !== 0)) { return invalid(); }
  return encoded;
}
export function person(value: unknown): Person {
  const row = record(value, ['type', 'void_id', 'auth_public_key']);
  return {void_id: voidId(row.void_id), auth_public_key: base64(row.auth_public_key, 32, 32)};
}
export function serverFrame(value: unknown): ServerFrame {
  const row = record(value);
  if (Object.values(row).some(item => typeof item !== 'string')) { return invalid(); }
  const type = string(row.type);
  switch (type) {
    case 'auth_challenge':
      record(row, ['type', 'challenge_id', 'message', 'expires_at']);
      return {type, challenge_id: exchangeId(row.challenge_id), message: base64(row.message, 4096), expires_at: timestamp(row.expires_at)};
    case 'auth_ok': record(row, ['type', 'void_id']); return {type, void_id: voidId(row.void_id)};
    case 'person': return {type, ...person(row)};
    case 'error': {
      record(row, ['type', 'code']); const code = string(row.code);
      if (!/^[a-z_]{1,64}$/.test(code)) { return invalid(); }
      return {type, code};
    }
    case 'prekeys_published':
      record(row, ['type', 'publication_id', 'ec_remaining', 'kyber_remaining']);
      return {type, publication_id: uuid(row.publication_id), ec_remaining: preKeyCount(row.ec_remaining), kyber_remaining: preKeyCount(row.kyber_remaining)};
    case 'prekey_bundle':
      record(row, ['type', 'request_id', 'void_id', 'payload_base64']);
      return {type, request_id: uuid(row.request_id), void_id: voidId(row.void_id), payload_base64: base64(row.payload_base64, 262144)};
    case 'prekeys_error': {
      record(row, ['type', 'request_id', 'code']);
      const code = string(row.code);
      if (!['prekeys_unavailable', 'prekeys_conflict', 'prekeys_invalid', 'prekeys_capacity', 'prekey_claim_expired'].includes(code)) { return invalid(); }
      return {type, request_id: uuid(row.request_id), code: code as PreKeyErrorCode};
    }
    case 'online_result': {
      record(row, ['type', 'exchange_id', 'status']);
      const status = string(row.status) as 'forwarded' | 'unavailable' | 'overloaded' | 'failed';
      if (!['forwarded', 'unavailable', 'overloaded', 'failed'].includes(status)) { return invalid(); }
      return {type, exchange_id: exchangeId(row.exchange_id), status};
    }
    case 'online_event':
      record(row, ['type', 'exchange_id', 'from_void_id', 'payload_base64']);
      return {type, exchange_id: exchangeId(row.exchange_id), from_void_id: voidId(row.from_void_id), payload_base64: base64(row.payload_base64)};
    case 'event': {
      record(row, ['type', 'event_id', 'from_void_id', 'payload_base64', 'created_at', 'expires_at']);
      const created_at = timestamp(row.created_at); const expires_at = timestamp(row.expires_at);
      if (Date.parse(expires_at) <= Date.parse(created_at)) { return invalid(); }
      return {type, event_id: uuid(row.event_id), from_void_id: voidId(row.from_void_id), payload_base64: base64(row.payload_base64), created_at, expires_at};
    }
    case 'accepted': {
      record(row, ['type', 'event_id', 'created_at', 'expires_at']);
      const created_at = timestamp(row.created_at); const expires_at = timestamp(row.expires_at);
      if (Date.parse(expires_at) <= Date.parse(created_at)) { return invalid(); }
      return {type, event_id: uuid(row.event_id), created_at, expires_at};
    }
    case 'ack_ok': record(row, ['type', 'sender_void_id', 'event_id']); return {type, sender_void_id: voidId(row.sender_void_id), event_id: uuid(row.event_id)};
    default: return invalid();
  }
}
export function snapshot(value: unknown): MessagingSnapshot {
  const row = record(value);
  if (!Array.isArray(row.sessions) || !Array.isArray(row.outbox) || !Array.isArray(row.blockedVoidIds) || row.sessions.length > 4096 || row.outbox.length > 256) { return invalid(); }
  const sessions = row.sessions.map<MessagingSnapshot['sessions'][number]>(entry => {
    const item = record(entry);
    const status = item.status as MessagingSnapshot['sessions'][number]['status'];
    const direction = item.direction;
    if (!['pending', 'handshaking', 'active', 'declined', 'blocked'].includes(status) || (direction !== 'incoming' && direction !== 'outgoing')) { return invalid(); }
    if (typeof item.isRequest !== 'boolean' || typeof item.hasMessages !== 'boolean') { return invalid(); }
    const diagnosticCode = item.diagnosticCode == null ? null : string(item.diagnosticCode);
    if (diagnosticCode !== null && !/^[A-Z][A-Z0-9_]{0,95}$/.test(diagnosticCode)) { return invalid(); }
    return {id: uuid(item.id), peer: person(item.peer), status, direction, isRequest: item.isRequest, hasMessages: item.hasMessages, diagnosticCode};
  });
  const outbox = row.outbox.map<MessagingSnapshot['outbox'][number]>(entry => {
    const item = record(entry); const kind = item.kind; const status = item.status;
    const purpose = item.purpose;
    if ((kind !== 'relay' && kind !== 'message') || (status !== 'pending' && status !== 'failed') || (purpose !== 'setup' && purpose !== 'application')) { return invalid(); }
    return {id: uuid(item.id), kind, purpose, status, peerVoidId: voidId(item.peerVoidId), payloadBase64: base64(item.payloadBase64), ...(item.errorCode == null ? {} : {errorCode: string(item.errorCode)})};
  });
  if (new Set(sessions.map(item => item.id)).size !== sessions.length || new Set(outbox.map(item => item.id)).size !== outbox.length) { return invalid(); }
  const requests = row.prekeyRequests ?? [];
  if (!Array.isArray(requests) || requests.length > 4096) { return invalid(); }
  const prekeyRequests = requests.map(entry => {
    const item = record(entry, ['id', 'peer']);
    return {id: uuid(item.id), peer: person(item.peer)};
  });
  if (new Set(prekeyRequests.map(item => item.id)).size !== prekeyRequests.length ||
      new Set(prekeyRequests.map(item => item.peer.void_id)).size !== prekeyRequests.length) { return invalid(); }
  return {sessions, outbox, blockedVoidIds: row.blockedVoidIds.map(voidId), prekeyRequests};
}
