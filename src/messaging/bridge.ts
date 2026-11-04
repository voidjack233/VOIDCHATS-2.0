import {NativeModules, Platform} from 'react-native';
import {decodeConversation} from '../storage/decode';
import * as decode from './decode';
import {MessagingError, safeMessagingError} from './errors';
import type {MessagingDriver, Person, QueueEvent} from './types';

interface NativeModule {
  signAuthChallenge(message: string): Promise<string>;
  getState(): Promise<string>;
  logSendDiagnostics(conversationId: string): Promise<string>;
  openConversation(peer: string): Promise<string>;
  resumePending(): Promise<string>;
  getPreKeyPublication(): Promise<string>;
  completePreKeyPublication(id: string): Promise<string>;
  establishSignalSession(peer: string, bundleBase64: string): Promise<string>;
  blockPeer(id: string): Promise<string>;
  queueMessage(id: string, body: string): Promise<string>;
  completeOperation(id: string, response: string): Promise<string>;
  failOperation(id: string, code: string): Promise<string>;
  failPending(code: string, peerVoidId: string | null): Promise<string>;
  retryOperation(id: string): Promise<string>;
  applyOnline(fromId: string, exchangeId: string, payload: string, peer: string): Promise<string>;
  applyEvent(event: string, peer: string): Promise<string>;
}
export function createMessagingDriver(getModule: () => NativeModule | undefined, platform: string): MessagingDriver {
  const call = async (method: keyof NativeModule, args: Array<string | null> = []): Promise<unknown> => {
    const native = getModule();
    if (!['android', 'ios'].includes(platform) || !native || typeof native[method] !== 'function') { throw new MessagingError('native_unavailable'); }
    try {
      const value = await (native[method] as (...values: Array<string | null>) => Promise<string>)(...args);
      if (method === 'signAuthChallenge') { return decode.base64(value, 64, 64); }
      if (typeof value !== 'string' || value.length > 16 * 1024 * 1024) { throw new MessagingError('invalid_response'); }
      return JSON.parse(value) as unknown;
    } catch (error) { throw safeMessagingError(error); }
  };
  const mutation = async (method: keyof NativeModule, ...args: Array<string | null>): Promise<void> => { await call(method, args); };
  return {
    signAuthChallenge: async message => await call('signAuthChallenge', [decode.base64(message, 4096)]) as string,
    getState: async () => decode.snapshot(await call('getState')),
    logSendDiagnostics: async id => {
      const row = decode.record(await call('logSendDiagnostics', [decode.uuid(id)]), ['count']);
      const count = row.count;
      if (typeof count !== 'number' || !Number.isSafeInteger(count) || count < 0 || count > 128) { throw new MessagingError('invalid_response'); }
      return count;
    },
    openConversation: async (peer: Person) => decodeConversation(await call('openConversation', [JSON.stringify(decode.person(peer))])),
    resumePending: () => mutation('resumePending'),
    getPreKeyPublication: async () => {
      const value = await call('getPreKeyPublication');
      if (value === null) { return null; }
      const row = decode.record(value, ['id', 'payloadBase64']);
      return {id: decode.uuid(row.id), payloadBase64: decode.base64(row.payloadBase64)};
    },
    completePreKeyPublication: id => mutation('completePreKeyPublication', decode.uuid(id)),
    establishSignalSession: (peer, bundleBase64) => mutation('establishSignalSession', JSON.stringify(decode.person(peer)), decode.base64(bundleBase64)),
    blockPeer: id => mutation('blockPeer', decode.voidId(id)),
    queueMessage: (id, body) => mutation('queueMessage', decode.uuid(id), body),
    completeOperation: (id, response) => mutation('completeOperation', decode.uuid(id), JSON.stringify(response)),
    failOperation: (id, code) => mutation('failOperation', decode.uuid(id), code),
    failPending: (code, peerVoidId) => mutation('failPending', code, peerVoidId === undefined ? null : decode.voidId(peerVoidId)),
    retryOperation: id => mutation('retryOperation', decode.uuid(id)),
    applyOnline: async () => { throw new MessagingError('unsupported_message'); },
    applyEvent: async (event: QueueEvent, peer: Person) => {
      const row = decode.record(await call('applyEvent', [JSON.stringify(event), JSON.stringify(decode.person(peer))]));
      if (typeof row.changed !== 'boolean' || row.event_id !== event.event_id || row.sender_void_id !== event.from_void_id) { throw new MessagingError('invalid_response'); }
      const diagnosticCode = row.diagnosticCode == null ? null : decode.string(row.diagnosticCode);
      if (diagnosticCode !== null && !/^[A-Z][A-Z0-9_]{0,95}$/.test(diagnosticCode)) { throw new MessagingError('invalid_response'); }
      return {event_id: decode.uuid(row.event_id), sender_void_id: decode.voidId(row.sender_void_id), changed: row.changed, diagnosticCode};
    },
  };
}
export const nativeMessaging = createMessagingDriver(() => NativeModules.VoidMessaging as NativeModule | undefined, Platform.OS);
