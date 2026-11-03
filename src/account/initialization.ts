import {AccountError, type AccountRegistration, type AccountStage, type RegistrationChallenge} from './client';
import type {AccountBinding} from '../storage/bridge';
import type {LocalIdentity} from '../storage/types';

export interface InitializationDependencies {
  client: {
    base: string;
    challenge(publicKey: string): Promise<RegistrationChallenge>;
    register(challengeId: string, signature: string): Promise<AccountRegistration>;
  };
  getIdentity(): LocalIdentity | null;
  prepare(apiUrl: string): Promise<LocalIdentity>;
  sign(messageBase64: string): Promise<string>;
  complete(account: AccountBinding): Promise<void>;
  onStage(stage: AccountStage): void;
}

/** One in-flight setup; a received account is retried locally before any new POST. */
export class AccountInitializer {
  private active: Promise<void> | null = null;
  private pending: AccountBinding | null = null;
  constructor(private readonly deps: InitializationDependencies) {}

  get hasPendingRegistration(): boolean { return this.pending !== null; }

  initialize = (): Promise<void> => {
    if (this.active) { return this.active; }
    const action = this.run();
    this.active = action;
    action.finally(() => { this.active = null; }).catch(() => {});
    return action;
  };

  private async save() {
    this.deps.onStage('saving');
    try { await this.deps.complete(this.pending!); }
    catch { throw new AccountError('save_failed'); }
    this.pending = null;
    this.deps.onStage('ready');
  }

  private async run() {
    if (this.pending) { await this.save(); return; }
    const {client} = this.deps;
    const existing = this.deps.getIdentity();
    if (existing?.accountVoidId) {
      if (existing.accountApiUrl !== client.base) { throw new AccountError('invalid_configuration'); }
      this.deps.onStage('ready'); return;
    }
    this.deps.onStage('preparing');
    const identity = await this.deps.prepare(client.base);
    if (identity.accountVoidId) {
      if (identity.accountApiUrl !== client.base) { throw new AccountError('invalid_configuration'); }
      this.deps.onStage('ready'); return;
    }
    this.deps.onStage('challenge');
    const challenge = await client.challenge(identity.signaturePublicKeyBase64);
    this.deps.onStage('signing');
    const signature = await this.deps.sign(challenge.message);
    this.deps.onStage('registering');
    const account = await client.register(challenge.challenge_id, signature);
    this.pending = {...account, api_url: client.base, auth_public_key: identity.signaturePublicKeyBase64};
    await this.save();
  }
}
