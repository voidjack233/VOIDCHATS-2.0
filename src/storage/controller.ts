import {sanitizeStorageError, StorageError, type AccountBinding, type StorageDriver} from './bridge';
import {createRepositories} from './repositories';
import type {LocalIdentity, NativeStorageSnapshot, StorageErrorCode, StorageStatus} from './types';

export interface StorageState {
  status: StorageStatus;
  message: string | null;
  errorCode: StorageErrorCode | null;
  identity: LocalIdentity | null;
  schemaVersion: number | null;
  revision: number;
}

/** Serializes lifecycle changes and publishes one revision after committed writes. */
export class StorageController {
  private state: StorageState = {status: 'opening', message: null, errorCode: null, identity: null, schemaVersion: null, revision: 0};
  private listeners = new Set<() => void>();
  private pending: Promise<void> = Promise.resolve();
  readonly repositories;
  constructor(private readonly driver: StorageDriver) {
    this.repositories = createRepositories(driver, () => this.publish({revision: this.state.revision + 1}), () => {
      if (this.state.status !== 'ready') { throw new StorageError(this.state.status === 'not_initialized' ? 'NOT_INITIALIZED' : 'LOCKED'); }
    }, error => {
      if (['LOCKED', 'KEY_UNAVAILABLE', 'CORRUPT', 'ENCRYPTION_UNAVAILABLE', 'DATABASE_ERROR', 'INVALID_NATIVE_RESPONSE', 'NATIVE_MODULE_UNAVAILABLE', 'NATIVE_MODULE_INCOMPATIBLE'].includes(error.code)) { this.failure(error); }
    });
  }
  getSnapshot = (): StorageState => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private publish(change: Partial<StorageState>) {
    this.state = {...this.state, ...change};
    for (const listener of this.listeners) {
      try { listener(); } catch { /* A view cannot turn a committed native write into failure. */ }
    }
  }
  private failure(error: StorageError) {
    this.publish({status: error.code === 'LOCKED' ? 'locked' : 'error', message: error.message, errorCode: error.code, identity: null, revision: this.state.revision + 1});
  }
  private apply(snapshot: NativeStorageSnapshot) {
    if (snapshot.status === 'error') {
      throw sanitizeStorageError({code: snapshot.errorCode});
    }
    this.publish({status: snapshot.status === 'open' ? 'ready' : snapshot.status,
      message: null, errorCode: null, identity: snapshot.status === 'open' ? snapshot.identity : null,
      schemaVersion: snapshot.schemaVersion, revision: this.state.revision + 1});
  }
  private run(action: () => Promise<NativeStorageSnapshot>): Promise<void> {
    const result = this.pending.then(async () => {
      this.publish({status: 'opening', identity: null, message: null, errorCode: null});
      try { this.apply(await action()); } catch (value) {
        const error = sanitizeStorageError(value); this.failure(error); throw error;
      }
    });
    this.pending = result.catch(() => {});
    return result;
  }
  /** Opens existing encrypted data only. No identity creation or regeneration. */
  openExisting = () => this.run(() => this.driver.open());
  /** Native messaging calls this only after its local transaction commits. */
  notifyCommitted = (localDeviceId: string) => {
    if (this.state.status === 'ready' && this.state.identity?.localDeviceId === localDeviceId) {
      this.publish({revision: this.state.revision + 1});
    }
  };
  createDevelopmentStorage = () => this.run(() => this.driver.initializeDevelopment());
  prepareAccountRegistration = async (apiUrl: string): Promise<LocalIdentity> => {
    let identity: LocalIdentity | null = null;
    await this.run(async () => {
      identity = await this.driver.prepareAccountRegistration(apiUrl);
      const snapshot = await this.driver.getStatus();
      if (snapshot.status !== 'open' || snapshot.identity?.localDeviceId !== identity.localDeviceId ||
          snapshot.identity.signaturePublicKeyBase64 !== identity.signaturePublicKeyBase64) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
      return snapshot;
    });
    return identity!;
  };
  signAccountChallenge = async (message: string): Promise<string> => {
    if (this.state.status !== 'ready') { throw new StorageError('LOCKED'); }
    return this.driver.signAccountChallenge(message);
  };
  completeAccountRegistration = (account: AccountBinding) => this.run(async () => {
    const snapshot = await this.driver.completeAccountRegistration(account);
    if (snapshot.status !== 'open' || snapshot.identity?.accountVoidId !== account.void_id ||
        snapshot.identity.accountApiUrl !== account.api_url ||
        snapshot.identity.signaturePublicKeyBase64 !== account.auth_public_key) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
    return snapshot;
  });
  closeStore = () => this.run(() => this.driver.close());
  /** The caller must first obtain explicit user confirmation. */
  removeLocalData = () => this.run(() => this.driver.removeLocalData());
}

