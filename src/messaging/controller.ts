import type {ConversationRecord, LocalIdentity} from '../storage/types';
import {MessagingClient, type Socket} from './client';
import {MessagingError, retryable, safeMessagingError} from './errors';
import type {PreKeyDirectory} from './prekeys';
import type {MessagingDriver, MessagingSnapshot, OnlineEvent, Person, QueueEvent} from './types';

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
  private inbound: Promise<void> = Promise.resolve();
  private inboundCount = 0;
  private readonly client: MessagingClient;
  constructor(private readonly options: Options) {
    this.client = new MessagingClient({...options,
      signAuthChallenge: message => options.driver.signAuthChallenge(message),
      onDisconnect: error => { if (this.active) { this.publish({status: 'offline', error: error.message}); this.schedule(); } },
      onEvent: event => this.receive(event),
    });
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
    this.refresh().catch(() => {});
  };
  stop = () => {
    this.active = false; this.generation++; this.scope = '';
    if (this.timer) { clearTimeout(this.timer); this.timer = null; }
    this.client.reset(); this.refreshing = null; this.flushing = null;
    this.inbound = Promise.resolve(); this.inboundCount = 0;
    this.publish(empty());
  };
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
        await this.reload(generation, true);
        if (!this.current(generation)) { return; }
        await this.client.connect();
        if (!this.current(generation)) { return; }
        this.backoff = 1000; this.publish({status: 'online', error: null});
        // Native Signal sessions can encrypt saved drafts without a directory
        // request. A first contact requires an explicitly supplied public bundle.
        await this.options.driver.resumePending();
        await this.reload(generation, true);
        if (!this.current(generation)) { return; }
        await this.flush();
      } catch (value) {
        if (!this.current(generation)) { return; }
        const error = safeMessagingError(value);
        this.publish({status: 'offline', error: error.message}); this.schedule(); throw error;
      }
    })();
    this.refreshing = action;
    action.finally(() => { if (this.refreshing === action) { this.refreshing = null; } }).catch(() => {});
    return action;
  };
  private async prepareSignalSessions(generation: number): Promise<MessagingError | null> {
    const directory = this.options.prekeys;
    if (!directory) {
      return this.state.prekeyRequests?.length ? new MessagingError('prekeys_unavailable') : null;
    }
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
      for (const request of this.state.prekeyRequests ?? []) {
        const peer = await this.client.lookup(request.peer.void_id);
        if (!this.current(generation)) { return null; }
        if (peer.auth_public_key !== request.peer.auth_public_key) { throw new MessagingError('identity_changed', 'PEER_KEY_CHANGED'); }
        const bundle = await directory.claim({...request, peer});
        if (!this.current(generation)) { return null; }
        await this.options.driver.establishSignalSession(peer, bundle);
        if (!this.current(generation)) { return null; }
      }
      await this.options.driver.resumePending();
      await this.reload(generation, true);
      return null;
    } catch (value) { return safeMessagingError(value); }
  }
  private flush(): Promise<void> {
    if (this.flushing) { return this.flushing; }
    const generation = this.generation;
    const action = (async () => {
      const prekeyError = await this.prepareSignalSessions(generation);
      if (!this.current(generation)) { return; }
      const attempted = new Set<string>();
      while (this.current(generation)) {
        const op = this.state.outbox.find(item => item.status === 'pending' && !attempted.has(item.id));
        if (!op) { break; }
        attempted.add(op.id);
        try {
          if (op.kind !== 'message' || op.purpose !== 'application') { throw new MessagingError('unsupported_message'); }
          const result = await this.client.send(op.id, op.peerVoidId, op.payloadBase64);
          if (!this.current(generation)) { return; }
          await this.options.driver.completeOperation(op.id, result);
          await this.reload(generation, true);
          if (this.current(generation)) { this.publish({error: null}); }
        } catch (value) {
          if (!this.current(generation)) { return; }
          const error = safeMessagingError(value);
          this.publish({error: error.message});
          if (retryable(error)) { this.schedule(); break; }
          // duplicate_event is uncertain acceptance, never a sent/delivered receipt.
          await this.options.driver.failOperation(op.id, error.code);
          await this.reload(generation, true);
        }
      }
      if (this.current(generation) && prekeyError) {
        this.publish({error: prekeyError.message});
        if (retryable(prekeyError)) { this.schedule(); }
      }
      if (this.current(generation) && this.state.outbox.some(item => item.status === 'pending')) { this.schedule(5000); }
    })();
    this.flushing = action;
    action.finally(() => { if (this.flushing === action) { this.flushing = null; } }).catch(() => {});
    return action;
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
        const peer = await this.client.lookup(event.from_void_id);
        if (!this.current(generation)) { return; }
        const ack = await this.options.driver.applyEvent(event, peer);
        if (!this.current(generation)) { return; }
        if (ack.diagnosticCode) { incomingError = safeMessagingError({code: ack.diagnosticCode}); }
        await this.reload(generation, ack.changed);
        // The native result is returned only after history/dedup commits.
        await this.client.ack({sender_void_id: ack.sender_void_id, event_id: ack.event_id});
      }
      if (this.current(generation)) { await this.flush(); }
      if (this.current(generation) && incomingError) { this.publish({error: incomingError.message}); }
    }).catch(value => {
      if (this.current(generation)) {
        const error = safeMessagingError(value);
        this.publish({error: error.message});
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
  blockPeer = (id: string) => this.mutate(() => this.options.driver.blockPeer(id));
  sendMessage = (id: string, body: string) => this.mutate(() => this.options.driver.queueMessage(id, body));
  retryOperation = (id: string) => this.mutate(() => this.options.driver.retryOperation(id));
  retryMessage = this.retryOperation;
}
