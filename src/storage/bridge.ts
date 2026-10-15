import type {NativeStorageModule, NativeStorageSnapshot, StorageErrorCode, StorageOperation} from './types';

const messages: Record<StorageErrorCode, string> = {
  UNSUPPORTED_PLATFORM: 'Encrypted local storage is available in the Android native build.',
  NATIVE_MODULE_UNAVAILABLE: 'Install the Android native build to use encrypted local storage.',
  NATIVE_MODULE_INCOMPATIBLE: 'Rebuild the Android app to update local storage.',
  NOT_INITIALIZED: 'Local storage has not been initialized.',
  LOCKED: 'Local storage is locked.',
  KEY_UNAVAILABLE: 'The device storage key is unavailable. Existing data was preserved.',
  CORRUPT: 'Encrypted local data could not be opened. Existing data was preserved.',
  ENCRYPTION_UNAVAILABLE: 'Encrypted local storage is unavailable on this device.',
  DATABASE_ERROR: 'Local storage could not complete the operation.',
  INVALID_INPUT: 'The local storage request is invalid.',
  NOT_FOUND: 'The local record was not found.',
  CONFLICT: 'The local record conflicts with existing data.',
  INVALID_NATIVE_RESPONSE: 'The native storage response is incompatible.',
  DEVELOPMENT_ONLY: 'Local development initialization is available only in debug builds.',
  OPERATION_FAILED: 'Local storage could not complete the operation.',
};

export class StorageError extends Error {
  readonly code: StorageErrorCode;
  constructor(code: StorageErrorCode) {
    super(messages[code]);
    this.name = 'StorageError';
    this.code = code;
  }
}

export function sanitizeStorageError(value: unknown): StorageError {
  if (value instanceof StorageError) { return value; }
  const code = value && typeof value === 'object' && 'code' in value ? value.code : undefined;
  return new StorageError(typeof code === 'string' && Object.prototype.hasOwnProperty.call(messages, code) ? code as StorageErrorCode : 'OPERATION_FAILED');
}

export interface StorageDriver {
  getStatus(): Promise<NativeStorageSnapshot>;
  open(): Promise<NativeStorageSnapshot>;
  initializeDevelopment(): Promise<NativeStorageSnapshot>;
  close(): Promise<NativeStorageSnapshot>;
  removeLocalData(): Promise<NativeStorageSnapshot>;
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
  return {
    getStatus: async () => decodeSnapshot(await invoke(native => native.getStatus())),
    open: async () => decodeSnapshot(await invoke(native => native.open())),
    initializeDevelopment: async () => {
      if (!development) { throw new StorageError('DEVELOPMENT_ONLY'); }
      return decodeSnapshot(await invoke(native => native.initializeDevelopment()));
    },
    close: async () => decodeSnapshot(await invoke(native => native.close())),
    removeLocalData: async () => decodeSnapshot(await invoke(native => native.removeLocalData())),
    execute: async (operation, payload) => {
      if (!operations.has(operation)) { throw new StorageError('INVALID_INPUT'); }
      return invoke(native => native.execute(operation, JSON.stringify(payload)));
    },
  };
}

