import type {ConversationRecord} from '../storage/types';
import type {PreKeyPublication, PreKeyRequest} from './prekeys';

export interface Person {void_id: string; auth_public_key: string}
export type RemotePerson = Person;
export interface OnlineEvent {type: 'online_event'; exchange_id: string; from_void_id: string; payload_base64: string}
export interface QueueEvent {type: 'event'; event_id: string; from_void_id: string; payload_base64: string; created_at: string; expires_at: string}
export interface Accepted {type: 'accepted'; event_id: string; created_at: string; expires_at: string}
export interface OnlineResult {type: 'online_result'; exchange_id: string; status: 'forwarded' | 'unavailable' | 'overloaded' | 'failed'}
export interface RecipientAck {sender_void_id: string; event_id: string}
export type PreKeyErrorCode = 'prekeys_unavailable' | 'prekeys_conflict' | 'prekeys_invalid' | 'prekeys_capacity' | 'prekey_claim_expired';
export interface PreKeysPublished {type: 'prekeys_published'; publication_id: string; ec_remaining: string; kyber_remaining: string}
export interface PreKeyBundleFrame {type: 'prekey_bundle'; request_id: string; void_id: string; payload_base64: string}
export type ServerFrame =
  | {type: 'auth_challenge'; challenge_id: string; message: string; expires_at: string}
  | {type: 'auth_ok'; void_id: string}
  | ({type: 'person'} & Person)
  | {type: 'error'; code: string}
  | OnlineEvent | QueueEvent | Accepted | OnlineResult
  | PreKeysPublished | PreKeyBundleFrame
  | {type: 'prekeys_error'; request_id: string; code: PreKeyErrorCode}
  | ({type: 'ack_ok'} & RecipientAck);
export interface LocalSession {
  id: string; peer: Person; direction: 'incoming' | 'outgoing';
  status: 'pending' | 'handshaking' | 'active' | 'declined' | 'blocked';
  isRequest: boolean;
  hasMessages: boolean;
  diagnosticCode: string | null;
}
export interface NativeOperation {
  id: string; kind: 'relay' | 'message'; peerVoidId: string; payloadBase64: string;
  purpose: 'setup' | 'application';
  status: 'pending' | 'failed'; errorCode?: string;
}
export interface MessagingSnapshot {
  sessions: LocalSession[]; outbox: NativeOperation[]; blockedVoidIds: string[];
  prekeyRequests?: PreKeyRequest[];
}
export interface MessagingDriver {
  signAuthChallenge(message: string): Promise<string>;
  getState(): Promise<MessagingSnapshot>;
  openConversation(peer: Person): Promise<ConversationRecord>;
  resumePending(): Promise<void>;
  getPreKeyPublication(): Promise<PreKeyPublication | null>;
  completePreKeyPublication(id: string): Promise<void>;
  establishSignalSession(peer: Person, bundleBase64: string): Promise<void>;
  blockPeer(voidId: string): Promise<void>;
  queueMessage(id: string, body: string): Promise<void>;
  completeOperation(id: string, response: unknown): Promise<void>;
  applyOnline(fromId: string, exchangeId: string, payload: string, peer: Person): Promise<void>;
  applyEvent(event: QueueEvent, peer: Person): Promise<RecipientAck & {changed: boolean; diagnosticCode?: string | null}>;
  failOperation(id: string, code: string): Promise<void>;
  retryOperation(id: string): Promise<void>;
}
