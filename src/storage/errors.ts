import type {StorageErrorCode} from './types';

const messages: Record<StorageErrorCode, string> = {
  UNSUPPORTED_PLATFORM: 'Encrypted local storage is available in the Android native build.',
  NATIVE_MODULE_UNAVAILABLE: 'Install the VOID Android app to set up an account. Expo Go supports UI preview only.',
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
