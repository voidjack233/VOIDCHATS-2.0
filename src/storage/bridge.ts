import type {LocalIdentity, NativeStorageModule, NativeStorageSnapshot, StorageOperation} from './types';
import {decodeIdentity} from './decode';
import {sanitizeStorageError, StorageError} from './errors';
export {sanitizeStorageError, StorageError} from './errors';

export interface AccountBinding {
  void_id: string;
  created_at: string;
  api_url: string;
  auth_public_key: string;
}

export interface StorageDriver {
  getStatus(): Promise<NativeStorageSnapshot>;
  open(): Promise<NativeStorageSnapshot>;
  initializeDevelopment(): Promise<NativeStorageSnapshot>;
  close(): Promise<NativeStorageSnapshot>;
  removeLocalData(): Promise<NativeStorageSnapshot>;
  prepareAccountRegistration(apiUrl: string): Promise<LocalIdentity>;
  signAccountChallenge(messageBase64: string): Promise<string>;
  completeAccountRegistration(account: AccountBinding): Promise<NativeStorageSnapshot>;
  execute(operation: StorageOperation, payload: object): Promise<unknown>;
}

const methods: Array<keyof NativeStorageModule> = ['getStatus', 'open', 'initializeDevelopment', 'close', 'removeLocalData', 'getLocalIdentity', 'execute'];
const operations = new Set<StorageOperation>([
  'conversations.list', 'conversations.page', 'conversations.get', 'conversations.save', 'conversations.markRead',
  'messages.page', 'messages.get', 'messages.save', 'messages.edit', 'messages.delete',
  'messages.setDeliveryState', 'messages.setReaction', 'messages.setReceipt',
  'contacts.list', 'contacts.get', 'contacts.save', 'contacts.remove',
  'requests.list', 'requests.save', 'requests.setStatus',
]);

function parse(value: unknown): unknown {
  if (typeof value !== 'string') { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  try { return JSON.parse(value) as unknown; } catch { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
}

/** Lazy lookup; importing this facade never creates a database or identity. */
export function createStorageDriver(
  resolve: () => NativeStorageModule | undefined,
  platform: string,
  development: boolean,
  decodeSnapshot: (value: unknown) => NativeStorageSnapshot,
): StorageDriver {
  const module = () => {
    if (platform !== 'android') { throw new StorageError('UNSUPPORTED_PLATFORM'); }
    const native = resolve();
    if (!native) { throw new StorageError('NATIVE_MODULE_UNAVAILABLE'); }
    if (methods.some(method => typeof native[method] !== 'function')) { throw new StorageError('NATIVE_MODULE_INCOMPATIBLE'); }
    return native;
  };
  const invoke = async (call: (native: NativeStorageModule) => Promise<string>) => {
    try { return parse(await call(module())); } catch (error) { throw sanitizeStorageError(error); }
  };
  const registrationModule = () => {
    const native = module();
    if (['prepareAccountRegistration', 'signAccountChallenge', 'completeAccountRegistration'].some(method => typeof native[method as keyof NativeStorageModule] !== 'function')) {
      throw new StorageError('NATIVE_MODULE_INCOMPATIBLE');
    }
    return native;
  };
  return {
    getStatus: async () => decodeSnapshot(await invoke(native => native.getStatus())),
    open: async () => decodeSnapshot(await invoke(native => native.open())),
    initializeDevelopment: async () => {
      if (!development) { throw new StorageError('DEVELOPMENT_ONLY'); }
      return decodeSnapshot(await invoke(native => native.initializeDevelopment()));
    },
    close: async () => decodeSnapshot(await invoke(native => native.close())),
    removeLocalData: async () => decodeSnapshot(await invoke(native => native.removeLocalData())),
    prepareAccountRegistration: async apiUrl => decodeIdentity(await invoke(() => registrationModule().prepareAccountRegistration(apiUrl))),
    signAccountChallenge: async message => {
      try {
        const signature = await registrationModule().signAccountChallenge(message);
        if (typeof signature !== 'string' || !/^[A-Za-z0-9+/]{86}==$/.test(signature)) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
        return signature;
      } catch (error) { throw sanitizeStorageError(error); }
    },
    completeAccountRegistration: async account => decodeSnapshot(await invoke(() => registrationModule().completeAccountRegistration(JSON.stringify(account)))),
    execute: async (operation, payload) => {
      if (!operations.has(operation)) { throw new StorageError('INVALID_INPUT'); }
      return invoke(native => native.execute(operation, JSON.stringify(payload)));
    },
  };
}

