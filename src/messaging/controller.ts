import type {ConversationRecord, LocalIdentity} from '../storage/types';
import {MessagingClient, type Socket} from './client';
import {MessagingError, retryable, safeMessagingError} from './errors';
import {logMessagingDiagnostic} from './diagnostics';
import {createPreKeyDirectory, type PreKeyDirectory} from './prekeys';
import type {Accepted, MessagingDriver, MessagingSnapshot, OnlineEvent, Person, QueueEvent} from './types';

export interface MessagingState extends MessagingSnapshot {
  status: 'disabled' | 'connecting' | 'online' | 'offline'; error: string | null;
}
interface Options {
  apiUrl: string; driver: MessagingDriver; identity(): LocalIdentity | null;
  socketFactory(url: string): Socket; committed(localDeviceId: string): void;
  prekeys?: PreKeyDirectory;
}
const empty = (): MessagingState => ({status: 'disabled', error: null, sessions: [], outbox: [], blockedVoidIds: [], prekeyRequests: []});

/** Native transactions own Signal keys, sessions, outbox and committed inbox history. */
export class MessagingController {
  private state = empty();
  private listeners = new Set<() => void>();
  private active = false;
  private generation = 0;
  private scope = '';
  private timer: ReturnType<typeof setTimeout> | null = null;
  private backoff = 1000;
  private refreshing: Promise<void> | null = null;
  private flushing: Promise<void> | null = null;
  private pendingFailure: Promise<void> = Promise.resolve();
  private flushRequested = false;
  private inbound: Promise<void> = Promise.resolve();
  private inboundCount = 0;
  private ackPriority: {ready: Promise<void>; release(): void} | null = null;
  private receiptTimer: {id: ReturnType<typeof setTimeout>; at: number} | null = null;
  private receiptUploading = false;
  private receiptWakeRequested = false;
  private receiptNotBefore = 0;
  private readonly client: MessagingClient;
  private readonly prekeys: PreKeyDirectory;
  constructor(private readonly options: Options) {
    this.client = new MessagingClient({...options,
      signAuthChallenge: message => options.driver.signAuthChallenge(message),
      onDisconnect: error => {
        if (this.active) {
          const generation = this.generation;
          this.clearReceiptTimer();
          this.publish({status: 'offline', error: error.message});
          this.pendingFailure = this.options.driver.failPending(error.code);
          this.pendingFailure.then(() => this.reload(generation, true)).catch(value => {
            if (this.current(generation)) { this.publish({error: safeMessagingError(value).message}); }
          });
          this.schedule();
        }
      },
      onEvent: event => this.receive(event),
    });
    this.prekeys = options.prekeys ?? createPreKeyDirectory(this.client);
  }
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(patch: Partial<MessagingState>) {
    this.state = {...this.state, ...patch};
    for (const listener of this.listeners) { try { listener(); } catch { /* A view cannot invalidate a commit. */ } }
  }
  start = () => {
    this.stop();
    const identity = this.options.identity();
    if (!identity?.accountVoidId) { return; }
    this.active = true; this.scope = identity.localDeviceId; this.backoff = 1000;
    // A previous process may have ended before it could record send failure.
    // Reopening only reconnects; interrupted messages require explicit Resend.
    this.recordInterruptedSends(this.scope);
    this.refresh().catch(() => {});
  };
  stop = () => {
    const interruptedScope = this.active ? this.scope : null;
    this.active = false; this.generation++; this.scope = '';
    if (this.timer) { clearTimeout(this.timer); this.timer = null; }
    this.clearReceiptTimer();
    this.ackPriority?.release(); this.ackPriority = null;
    this.receiptUploading = false; this.receiptWakeRequested = false; this.receiptNotBefore = 0;
    this.client.reset(); this.refreshing = null; this.flushing = null; this.flushRequested = false;
    this.inbound = Promise.resolve(); this.inboundCount = 0;
    if (interruptedScope) { this.recordInterruptedSends(interruptedScope); }
    this.publish(empty());
  };
  private recordInterruptedSends(scope: string) {
    this.pendingFailure = this.options.driver.failPending('send_interrupted');
    this.pendingFailure.then(() => {
      logMessagingDiagnostic('pending_sends_failed', () => ({local_void_id: this.options.identity()?.accountVoidId, code: 'send_interrupted'}));
      this.options.committed(scope);
    }, () => {});
  }
  private current(generation: number): boolean { return this.active && generation === this.generation && this.options.identity()?.localDeviceId === this.scope; }
  private schedule(delay = this.backoff) {
    if (!this.active || this.timer) { return; }
    this.backoff = Math.min(this.backoff * 2, 30000);
    this.timer = setTimeout(() => { this.timer = null; this.refresh().catch(() => {}); }, delay);
  }
  private async reload(generation: number, committed = false) {
    const state = await this.options.driver.getState();
    if (!this.current(generation)) { return; }
    this.publish(state);
    if (committed) { this.options.committed(this.scope); }
  }
  refresh = (): Promise<void> => {
    if (!this.active) { return Promise.resolve(); }
    if (this.refreshing) { return this.refreshing; }
    const generation = this.generation;
    const action = (async () => {
      if (this.timer) { clearTimeout(this.timer); this.timer = null; }
      if (this.state.status !== 'online') { this.publish({status: 'connecting'}); }
      try {
        await this.pendingFailure;
        if (!this.current(generation)) { return; }
        await this.reload(generation, true);
        if (!this.current(generation)) { return; }
        await this.client.connect();
        if (!this.current(generation)) { return; }
        this.backoff = 1000; this.publish({status: 'online', error: null});
        this.wakeReceiptUploads(generation);
        // Encrypt drafts for existing sessions before publishing or claiming
        // public prekeys for new contacts over this authenticated connection.
        await this.options.driver.resumePending();
        await this.reload(generation, true);
        if (!this.current(generation)) { return; }
        await this.flush();
      } catch (value) {
        if (!this.current(generation)) { return; }
        const error = safeMessagingError(value);
        this.publish({status: 'offline', error: error.message});
        await this.options.driver.failPending(error.code);
        await this.reload(generation, true);
        this.schedule(); throw error;
      }
    })();
    this.refreshing = action;
    action.finally(() => { if (this.refreshing === action) { this.refreshing = null; } }).catch(() => {});
    return action;
  };
  reconnect = (): Promise<void> => {
    if (!this.active) { return Promise.resolve(); }
    logMessagingDiagnostic('reconnect', () => ({local_void_id: this.options.identity()?.accountVoidId}));
    // Restart the connection lifecycle so an authenticated socket cannot make
    // an explicit reconnect return without replaying queued recipient events.
    this.start();
    return this.refreshing ?? Promise.resolve();
  };
  private async prepareSignalSessions(generation: number): Promise<MessagingError | null> {
    const directory = this.prekeys;
    try {
      await this.client.connect();
      if (!this.current(generation)) { return null; }
      const publication = await this.options.driver.getPreKeyPublication();
      if (!this.current(generation)) { return null; }
      if (publication) {
        await directory.publish(publication);
        if (!this.current(generation)) { return null; }
        await this.options.driver.completePreKeyPublication(publication.id);
      }
    } catch (value) {
      if (!this.current(generation)) { return null; }
      const error = safeMessagingError(value);
      logMessagingDiagnostic('prekey_setup_failed', () => ({local_void_id: this.options.identity()?.accountVoidId, code: error.code, diagnostic_code: error.diagnosticCode}));
      await this.options.driver.failPending(error.code);
      await this.reload(generation, true);
      return error;
    }
    let firstError: MessagingError | null = null;
    for (const request of this.state.prekeyRequests ?? []) {
      try {
        const peer = await this.client.lookup(request.peer.void_id);
        if (!this.current(generation)) { return null; }
        if (peer.auth_public_key !== request.peer.auth_public_key) { throw new MessagingError('identity_changed', 'PEER_KEY_CHANGED'); }
        const bundle = await directory.claim({...request, peer});
        if (!this.current(generation)) { return null; }
        await this.options.driver.establishSignalSession(peer, bundle);
        if (!this.current(generation)) { return null; }
      } catch (value) {
        if (!this.current(generation)) { return null; }
        const error = safeMessagingError(value);
        logMessagingDiagnostic('prekey_setup_failed', () => ({local_void_id: this.options.identity()?.accountVoidId,
          request_id: request.id, recipient_void_id: request.peer.void_id, code: error.code, diagnostic_code: error.diagnosticCode}));
        await this.options.driver.failPending(error.code, request.peer.void_id);
        await this.reload(generation, true);
        firstError ??= error;
      }
    }
    await this.options.driver.resumePending();
    await this.reload(generation, true);
    return firstError;
  }
  private flush(): Promise<void> {
    if (this.flushing) { this.flushRequested = true; return this.flushing; }
    const generation = this.generation;
    const action = (async () => {
      let prepared = false;
      let prekeyError: MessagingError | null = null;
      const attempted = new Set<string>();
      while (this.current(generation)) {
        const op = this.state.outbox.find(item => item.status === 'pending' && !attempted.has(item.id));
        if (!op) {
          if (prepared) { break; }
          // Existing ciphertext goes straight to send, without waiting for
          // public-key publication or another contact's session setup.
          prepared = true;
          prekeyError = await this.prepareSignalSessions(generation);
          continue;
        }
        attempted.add(op.id);
        try {
          if (op.kind !== 'message' || op.purpose !== 'application') { throw new MessagingError('unsupported_message'); }
          const result = await this.client.send(op.id, op.peerVoidId, op.payloadBase64);
          if (!this.current(generation)) { return; }
          await this.options.driver.completeOperation(op.id, result);
          logMessagingDiagnostic('send_local_commit', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: op.id, recipient_void_id: op.peerVoidId}));
          await this.reload(generation, true);
          if (this.current(generation)) { this.publish({error: null}); }
        } catch (value) {
          if (!this.current(generation)) { return; }
          const error = safeMessagingError(value);
          this.publish({error: error.message});
          // duplicate_event is uncertain acceptance, never a sent/delivered receipt.
          await this.options.driver.failOperation(op.id, error.code);
          logMessagingDiagnostic('send_failed', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: op.id,
            recipient_void_id: op.peerVoidId, code: error.code, diagnostic_code: error.diagnosticCode}));
          await this.reload(generation, true);
        }
      }
      if (this.current(generation) && prekeyError) {
        this.publish({error: prekeyError.message});
      }
    })();
    this.flushing = action;
    action.finally(() => {
      if (this.flushing !== action) { return; }
      this.flushing = null;
      const requested = this.flushRequested; this.flushRequested = false;
      if (requested && this.current(generation)) {
        this.flush().catch(value => {
          if (this.current(generation)) { this.publish({error: safeMessagingError(value).message}); this.schedule(); }
        });
      }
    }).catch(() => {});
    return action;
  }
  private clearReceiptTimer() {
    if (this.receiptTimer) { clearTimeout(this.receiptTimer.id); this.receiptTimer = null; }
  }
  private canUploadReceipts(generation: number): boolean {
    return this.current(generation) && this.state.status === 'online';
  }
  private wakeReceiptUploads(generation: number, at = Date.now()) {
    const {driver} = this.options;
    if (!this.canUploadReceipts(generation) || !driver.claimReceiptUpload || !driver.completeReceiptUpload || !driver.stopReceiptUpload) { return; }
    if (this.receiptUploading) { this.receiptWakeRequested = true; return; }
    const scheduledAt = Math.max(at, this.receiptNotBefore);
    if (this.receiptTimer && this.receiptTimer.at <= scheduledAt) { return; }
    this.clearReceiptTimer();
    const id = setTimeout(() => {
      this.receiptTimer = null;
      this.uploadReceipt(generation).catch(() => {});
    }, Math.min(2_147_483_647, Math.max(0, scheduledAt - Date.now())));
    this.receiptTimer = {id, at: scheduledAt};
  }
  private async uploadReceipt(generation: number) {
    const {driver} = this.options;
    if (!this.canUploadReceipts(generation) || this.receiptUploading || !driver.claimReceiptUpload ||
        !driver.completeReceiptUpload || !driver.stopReceiptUpload) { return; }
    this.receiptUploading = true;
    let nextAttemptAt: number | null = null;
    try {
      // Yield only until inbound processing queues its ACK on the shared socket.
      while (this.ackPriority && this.canUploadReceipts(generation)) { await this.ackPriority.ready; }
      if (!this.canUploadReceipts(generation)) { return; }
      const {operation, nextAttemptAt: nativeNextAttemptAt} = await driver.claimReceiptUpload();
      nextAttemptAt = nativeNextAttemptAt;
      if (!operation) { return; }
      // A new inbound event can arrive while SQLCipher is preparing the upload.
      while (this.ackPriority && this.canUploadReceipts(generation)) { await this.ackPriority.ready; }
      if (!this.canUploadReceipts(generation)) { return; }
      nextAttemptAt = Date.now();
      this.receiptNotBefore = nextAttemptAt + 1000;
      if (this.state.blockedVoidIds.includes(operation.peerVoidId)) {
        await driver.stopReceiptUpload(operation.id);
        return;
      }
      let accepted: Accepted;
      try {
        accepted = await this.client.send(operation.id, operation.peerVoidId, operation.payloadBase64,
          () => this.canUploadReceipts(generation) && !this.state.blockedVoidIds.includes(operation.peerVoidId));
      } catch (value) {
        const error = safeMessagingError(value);
        if (this.current(generation) && (error.code === 'invalid_frame' || error.code === 'not_found')) {
          await driver.stopReceiptUpload(operation.id);
        }
        // Ambiguous acceptance leaves the same saved ciphertext eligible on the native schedule.
        throw error;
      }
      if (this.current(generation)) { await driver.completeReceiptUpload(operation.id, accepted); }
    } catch (value) {
      if (!this.current(generation)) { return; }
      const error = safeMessagingError(value);
      logMessagingDiagnostic('receipt_upload_failed', () => ({code: error.code, diagnostic_code: error.diagnosticCode}));
      // Give the connection time to recover without draining other ready receipts in a tight loop.
      this.receiptNotBefore = Date.now() + 5000;
      nextAttemptAt = this.receiptNotBefore;
    } finally {
      if (generation === this.generation) {
        this.receiptUploading = false;
        const requested = this.receiptWakeRequested; this.receiptWakeRequested = false;
        if (requested) { this.wakeReceiptUploads(generation); }
        else if (nextAttemptAt !== null) { this.wakeReceiptUploads(generation, nextAttemptAt); }
      }
    }
  }
  private receive(event: OnlineEvent | QueueEvent) {
    if (!this.active) { return; }
    if (this.inboundCount >= 32) { this.client.reset(); this.publish({status: 'offline', error: new MessagingError('overloaded').message}); this.schedule(); return; }
    const generation = this.generation; this.inboundCount++;
    this.inbound = this.inbound.then(async () => {
      if (!this.current(generation)) { return; }
      let incomingError: MessagingError | null = null;
      if (event.type === 'online_event') {
        throw new MessagingError('unsupported_message');
      } else {
        let release!: () => void;
        const ready = new Promise<void>(resolve => { release = resolve; });
        const priority = {ready, release};
        this.ackPriority = priority;
        let receiptQueued = false;
        let acknowledgement: Promise<void>;
        try {
          const peer = await this.client.lookup(event.from_void_id);
          if (!this.current(generation)) { return; }
          logMessagingDiagnostic('event_apply', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: event.event_id,
            sender_void_id: event.from_void_id, created_at: event.created_at, expires_at: event.expires_at}));
          const ack = await this.options.driver.applyEvent(event, peer);
          if (!this.current(generation)) { return; }
          receiptQueued = ack.receiptQueued === true;
          logMessagingDiagnostic('event_local_commit', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: ack.event_id,
            sender_void_id: ack.sender_void_id, changed: ack.changed}));
          if (ack.diagnosticCode) { incomingError = safeMessagingError({code: ack.diagnosticCode}); }
          await this.reload(generation, ack.changed);
          if (!this.current(generation)) { return; }
          // The native result is returned only after history/dedup commits.
          acknowledgement = this.client.ack({sender_void_id: ack.sender_void_id, event_id: ack.event_id});
        } finally {
          priority.release();
          if (this.ackPriority === priority) { this.ackPriority = null; }
          if (receiptQueued) { this.wakeReceiptUploads(generation); }
        }
        await acknowledgement;
      }
      if (this.current(generation)) { await this.flush(); }
      if (this.current(generation) && incomingError) { this.publish({error: incomingError.message}); }
    }).catch(value => {
      if (this.current(generation)) {
        const error = safeMessagingError(value);
        logMessagingDiagnostic('event_failed', () => ({local_void_id: this.options.identity()?.accountVoidId,
          event_id: event.type === 'event' ? event.event_id : undefined, sender_void_id: event.from_void_id,
          code: error.code, diagnostic_code: error.diagnosticCode}));
        this.publish({error: error.diagnosticCode ? `${error.message} Error code: ${error.diagnosticCode}` : error.message});
        // No ACK on failed processing; the server retains the queued event.
        // Reconnect also retries a failed post-commit ACK through native dedup.
        if (retryable(error) || error.code === 'storage') { this.client.reset(); this.publish({status: 'offline'}); this.schedule(); }
      }
    }).finally(() => { if (generation === this.generation) { this.inboundCount--; } });
  }
  lookup = async (id: string): Promise<Person> => {
    if (!this.active) { throw new MessagingError('not_registered'); }
    const generation = this.generation; const peer = await this.client.lookup(id);
    if (!this.current(generation)) { throw new MessagingError('not_registered'); }
    this.publish({status: 'online'});
    return peer;
  };
  private async mutate(action: () => Promise<void>) {
    if (!this.active) { throw new MessagingError('not_registered'); }
    const generation = this.generation;
    await action();
    if (!this.current(generation)) { return; }
    await this.reload(generation, true);
    this.flush().catch(value => { if (this.current(generation)) { this.publish({error: safeMessagingError(value).message}); this.schedule(); } });
  }
  openConversation = async (peer: Person): Promise<ConversationRecord> => {
    if (!this.active) { throw new MessagingError('not_registered'); }
    const generation = this.generation;
    const conversation = await this.options.driver.openConversation(peer);
    if (!this.current(generation)) { throw new MessagingError('not_registered'); }
    await this.reload(generation, true);
    return conversation;
  };
  logSendDiagnostics = async (id: string): Promise<number> => {
    if (!this.active) { throw new MessagingError('not_registered'); }
    return this.options.driver.logSendDiagnostics(id);
  };
  blockPeer = (id: string) => this.mutate(() => this.options.driver.blockPeer(id));
  sendMessage = (id: string, body: string) => this.mutate(() => this.options.driver.queueMessage(id, body));
  retryOperation = (id: string) => {
    logMessagingDiagnostic('retry', () => ({local_void_id: this.options.identity()?.accountVoidId, event_id: id}));
    return this.mutate(() => this.options.driver.retryOperation(id));
  };
  retryMessage = this.retryOperation;
}
