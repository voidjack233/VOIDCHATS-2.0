import {normalizeApiUrl} from '../account/client';
import {decodeBase64, isChallengeToken, usernameBase64} from '../account/validation';
import type {LocalIdentity} from '../storage/types';
import * as decode from './decode';
import {logMessagingDiagnostic} from './diagnostics';
import {MessagingError} from './errors';
import type {Accepted, OnlineEvent, OnlineResult, Person, PreKeyBundleFrame, PreKeysPublished, QueueEvent, RecipientAck, ServerFrame} from './types';

export interface Socket {
  onopen: (() => void) | null; onmessage: ((event: {data: unknown}) => void) | null;
  onerror: (() => void) | null; onclose: (() => void) | null;
  send(data: string): void; close(): void;
}
export interface MessagingClientOptions {
  apiUrl: string; identity(): LocalIdentity | null;
  signAuthChallenge(message: string): Promise<string>; socketFactory(url: string): Socket;
  onEvent(event: OnlineEvent | QueueEvent): void; onDisconnect(error: MessagingError): void;
  timeoutMs?: number;
}

/** Connection-scoped authentication. Errors have no correlation ID, so commands serialize. */
export class MessagingClient {
  readonly base: string;
  private socket: Socket | null = null;
  private authenticated = false;
  private connecting: Promise<void> | null = null;
  private cancelConnect: ((error: MessagingError) => void) | null = null;
  private generation = 0;
  private commands: Promise<unknown> = Promise.resolve();
  private waiting: {match(frame: ServerFrame): boolean; errorRequestId?: string; resolve(frame: ServerFrame): void; reject(error: MessagingError): void} | null = null;
  constructor(private readonly options: MessagingClientOptions) { this.base = normalizeApiUrl(options.apiUrl); }
  private identity(): LocalIdentity & {accountVoidId: string} {
    const identity = this.options.identity();
    if (!identity?.accountVoidId || identity.accountApiUrl !== this.base) { throw new MessagingError('not_registered'); }
    return identity as LocalIdentity & {accountVoidId: string};
  }
  private shutdown(error: MessagingError, notify: boolean) {
    if (notify) { logMessagingDiagnostic('socket_disconnect', () => ({local_void_id: this.options.identity()?.accountVoidId, code: error.code})); }
    this.generation++; this.authenticated = false;
    const socket = this.socket; this.socket = null;
    if (socket) { socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null; try { socket.close(); } catch { /* Already closed. */ } }
    const cancel = this.cancelConnect; this.cancelConnect = null; this.connecting = null; cancel?.(error);
    const waiting = this.waiting; this.waiting = null; waiting?.reject(error);
    if (notify) { this.options.onDisconnect(error); }
  }
  reset() {
    logMessagingDiagnostic('socket_reset', () => ({local_void_id: this.options.identity()?.accountVoidId}));
    this.shutdown(new MessagingError('unavailable'), false);
  }
  connect(): Promise<void> {
    const identity = this.identity();
    if (this.authenticated) { return Promise.resolve(); }
    if (this.connecting) { return this.connecting; }
    logMessagingDiagnostic('socket_connect', () => ({local_void_id: identity.accountVoidId}));
    const generation = this.generation;
    const promise = new Promise<void>((resolve, reject) => {
      let challenged = false; let proofSent = false;
      const timer = setTimeout(() => this.shutdown(new MessagingError('deadline_exceeded'), true), this.options.timeoutMs ?? 9500);
      this.cancelConnect = error => { clearTimeout(timer); reject(error); };
      try {
        const socket = this.options.socketFactory(this.base.replace(/^https:/, 'wss:') + '/ws');
        this.socket = socket;
        socket.onopen = () => {
          if (generation !== this.generation) { return; }
          try { socket.send(JSON.stringify({type: 'auth_begin', void_id: identity.accountVoidId})); }
          catch { this.shutdown(new MessagingError('unavailable'), true); }
        };
        socket.onerror = socket.onclose = () => { if (generation === this.generation) { this.shutdown(new MessagingError('unavailable'), true); } };
        socket.onmessage = event => {
          if (generation !== this.generation) { return; }
          try {
            if (typeof event.data !== 'string' || event.data.length > 1_410_000) { throw new MessagingError('invalid_response'); }
            const frame = decode.serverFrame(JSON.parse(event.data));
            if (!this.authenticated) {
              if (frame.type === 'error') { throw new MessagingError(frame.code); }
              if (frame.type === 'auth_challenge' && !challenged) {
                challenged = true;
                const expiry = Date.parse(frame.expires_at);
                if (!isChallengeToken(frame.challenge_id) || expiry <= Date.now() || expiry > Date.now() + 30000) { throw new MessagingError('invalid_response'); }
                const bytes = decodeBase64(frame.message);
                if (bytes.some(byte => byte > 127)) { throw new MessagingError('invalid_response'); }
                const message = String.fromCharCode(...bytes);
                const prefix = `VOID-NOTHINGNESS:authenticate:v2\n${frame.challenge_id}\n${identity.accountVoidId}\n${identity.signaturePublicKeyBase64}\n${frame.expires_at}\n${usernameBase64(this.base)}\n`;
                if (!message.startsWith(prefix) || !message.endsWith('\n') || !isChallengeToken(message.slice(prefix.length, -1))) { throw new MessagingError('invalid_response'); }
                this.options.signAuthChallenge(frame.message).then(signature => {
                  if (generation !== this.generation || this.identity().localDeviceId !== identity.localDeviceId) { return; }
                  if (expiry <= Date.now()) { throw new MessagingError('invalid_session'); }
                  proofSent = true;
                  socket.send(JSON.stringify({type: 'auth_proof', challenge_id: frame.challenge_id, signature: decode.base64(signature, 64, 64)}));
                }).catch(() => { if (generation === this.generation) { this.shutdown(new MessagingError('invalid_session'), true); } });
                return;
              }
              if (frame.type !== 'auth_ok' || !proofSent || frame.void_id !== identity.accountVoidId) { throw new MessagingError('invalid_response'); }
              clearTimeout(timer); this.cancelConnect = null; this.connecting = null; this.authenticated = true;
              logMessagingDiagnostic('auth_ok', () => ({local_void_id: identity.accountVoidId}));
              resolve();
              return;
            }
            if (frame.type === 'online_event' || frame.type === 'event') {
              if (frame.type === 'event') {
                logMessagingDiagnostic('event_received', () => ({local_void_id: identity.accountVoidId, event_id: frame.event_id,
                  sender_void_id: frame.from_void_id, created_at: frame.created_at, expires_at: frame.expires_at}));
              }
              this.options.onEvent(frame); return;
            }
            const waiting = this.waiting;
            if (frame.type === 'prekeys_error') {
              if (!waiting || waiting.errorRequestId !== frame.request_id) { throw new MessagingError('invalid_response'); }
              this.waiting = null; waiting.reject(new MessagingError(frame.code)); return;
            }
            if (frame.type === 'error' && waiting) { this.waiting = null; waiting.reject(new MessagingError(frame.code)); return; }
            if (!waiting || !waiting.match(frame)) { throw new MessagingError('invalid_response'); }
            this.waiting = null; waiting.resolve(frame);
          } catch (error) { this.shutdown(error instanceof MessagingError ? error : new MessagingError('invalid_response'), true); }
        };
      } catch { this.shutdown(new MessagingError('unavailable'), true); }
    });
    if (generation === this.generation && !this.authenticated) { this.connecting = promise; }
    return promise;
  }
  private command(command: object, match: (frame: ServerFrame) => boolean, errorRequestId?: string,
    canTransmit?: () => boolean): Promise<ServerFrame> {
    const generation = this.generation;
    const result = this.commands.then(async () => {
      if (generation !== this.generation) { throw new MessagingError('unavailable'); }
      await this.connect();
      if (generation !== this.generation || !this.authenticated || !this.socket) { throw new MessagingError('unavailable'); }
      if (canTransmit && !canTransmit()) { throw new MessagingError('unavailable'); }
      return new Promise<ServerFrame>((resolve, reject) => {
        const timer = setTimeout(() => this.shutdown(new MessagingError('deadline_exceeded'), true), this.options.timeoutMs ?? 20000);
        this.waiting = {match, errorRequestId, resolve: frame => { clearTimeout(timer); resolve(frame); }, reject: error => { clearTimeout(timer); reject(error); }};
        try {
          this.socket!.send(JSON.stringify(command));
          logMessagingDiagnostic('socket_transmit', () => {
            const metadata = command as {type?: string; event_id?: string; sender_void_id?: string;
              to_void_id?: string; void_id?: string; request_id?: string; publication_id?: string};
            return {local_void_id: this.options.identity()?.accountVoidId, command_type: metadata.type, event_id: metadata.event_id,
              sender_void_id: metadata.sender_void_id, recipient_void_id: metadata.to_void_id ?? metadata.void_id,
              request_id: metadata.request_id, publication_id: metadata.publication_id};
          });
        } catch { this.shutdown(new MessagingError('unavailable'), true); }
      });
    });
    this.commands = result.catch(() => {});
    return result;
  }
  async lookup(id: string): Promise<Person> {
    const void_id = decode.voidId(id);
    return decode.person(await this.command({type: 'lookup', void_id}, frame => frame.type === 'person' && frame.void_id === void_id));
  }
  async publishPreKeys(publication_id: string, payload_base64: string): Promise<PreKeysPublished> {
    decode.uuid(publication_id); decode.base64(payload_base64, 262144);
    logMessagingDiagnostic('prekeys_publish', () => ({local_void_id: this.options.identity()?.accountVoidId, publication_id}));
    const result = await this.command({type: 'prekeys_publish', publication_id, payload_base64},
      frame => frame.type === 'prekeys_published' && frame.publication_id === publication_id, publication_id) as PreKeysPublished;
    logMessagingDiagnostic('prekeys_published', () => ({local_void_id: this.options.identity()?.accountVoidId, publication_id}));
    return result;
  }
  async claimPreKeys(request_id: string, void_id: string): Promise<PreKeyBundleFrame> {
    decode.uuid(request_id); decode.voidId(void_id);
    logMessagingDiagnostic('prekeys_claim', () => ({local_void_id: this.options.identity()?.accountVoidId, request_id, recipient_void_id: void_id}));
    const result = await this.command({type: 'prekeys_claim', request_id, void_id},
      frame => frame.type === 'prekey_bundle' && frame.request_id === request_id && frame.void_id === void_id, request_id) as PreKeyBundleFrame;
    logMessagingDiagnostic('prekeys_claimed', () => ({local_void_id: this.options.identity()?.accountVoidId, request_id, recipient_void_id: void_id}));
    return result;
  }
  async relay(exchange_id: string, to_void_id: string, payload_base64: string): Promise<OnlineResult> {
    decode.exchangeId(exchange_id); decode.voidId(to_void_id); decode.base64(payload_base64);
    return await this.command({type: 'relay_online', exchange_id, to_void_id, payload_base64}, frame => frame.type === 'online_result' && frame.exchange_id === exchange_id) as OnlineResult;
  }
  async send(event_id: string, to_void_id: string, payload_base64: string, canTransmit?: () => boolean): Promise<Accepted> {
    decode.uuid(event_id); decode.voidId(to_void_id); decode.base64(payload_base64);
    logMessagingDiagnostic('send_submit', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id, recipient_void_id: to_void_id}));
    const result = await this.command({type: 'send', event_id, to_void_id, payload_base64},
      frame => frame.type === 'accepted' && frame.event_id === event_id, undefined, canTransmit) as Accepted;
    logMessagingDiagnostic('send_accepted', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id, recipient_void_id: to_void_id,
      created_at: result.created_at, expires_at: result.expires_at}));
    return result;
  }
  async ack(ack: RecipientAck): Promise<void> {
    decode.uuid(ack.event_id); decode.voidId(ack.sender_void_id);
    logMessagingDiagnostic('ack_submit', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: ack.event_id, sender_void_id: ack.sender_void_id}));
    await this.command({type: 'ack', ...ack}, frame => frame.type === 'ack_ok' && frame.event_id === ack.event_id && frame.sender_void_id === ack.sender_void_id);
    logMessagingDiagnostic('ack_ok', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: ack.event_id, sender_void_id: ack.sender_void_id}));
  }
}
