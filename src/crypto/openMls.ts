import {NativeModules, Platform} from 'react-native';
import type {
  OpenMls,
  OpenMlsAvailability,
  OpenMlsBridgeErrorCode,
} from './types';

export class OpenMlsBridgeError extends Error {
  readonly code: OpenMlsBridgeErrorCode;

  constructor(code: OpenMlsBridgeErrorCode, message: string) {
    super(message);
    this.name = 'OpenMlsBridgeError';
    this.code = code;
  }
}

const requiredMethods: ReadonlyArray<keyof OpenMls> = [
  'initialize',
  'createClient',
  'generateKeyPackage',
  'createGroup',
  'addMember',
  'mergePendingCommit',
  'processWelcome',
  'encrypt',
  'decrypt',
  'selfUpdate',
  'processCommit',
  'groupInfo',
  'suspendClient',
  'restoreClient',
  'releaseClient',
  'discardSnapshot',
];

/** Module registration only; call initialize() to verify the Rust library loads. */
export function getOpenMlsAvailability(): OpenMlsAvailability {
  if (Platform.OS !== 'android') {
    return {
      available: false,
      code: 'UNSUPPORTED_PLATFORM',
      message:
        'OpenMLS native integration currently supports Android development builds. iOS and web bindings are not available yet.',
    };
  }

  const nativeModule = NativeModules.VoidOpenMls as Partial<OpenMls> | undefined;
  if (!nativeModule) {
    return {
      available: false,
      code: 'NATIVE_MODULE_UNAVAILABLE',
      message:
        'The VoidOpenMls native module is unavailable. Install the custom Android development build; Expo Go cannot load this module.',
    };
  }

  if (requiredMethods.some(method => typeof nativeModule[method] !== 'function')) {
    return {
      available: false,
      code: 'NATIVE_MODULE_INCOMPATIBLE',
      message:
        'The installed VoidOpenMls module does not match this frontend. Rebuild and reinstall the Android development build.',
    };
  }

  return {available: true, platform: 'android'};
}

function nativeOpenMls(): OpenMls {
  const availability = getOpenMlsAvailability();
  if (!availability.available) {
    throw new OpenMlsBridgeError(availability.code, availability.message);
  }
  return NativeModules.VoidOpenMls as OpenMls;
}

/** Lazy native facade; importing it does not create identity material. */
export const openMls: OpenMls = {
  initialize: async () => nativeOpenMls().initialize(),
  createClient: async credentialIdentityBase64 =>
    nativeOpenMls().createClient(credentialIdentityBase64),
  generateKeyPackage: async clientHandle =>
    nativeOpenMls().generateKeyPackage(clientHandle),
  createGroup: async (clientHandle, groupIdBase64) =>
    nativeOpenMls().createGroup(clientHandle, groupIdBase64),
  addMember: async (clientHandle, groupIdBase64, keyPackageBase64) =>
    nativeOpenMls().addMember(clientHandle, groupIdBase64, keyPackageBase64),
  mergePendingCommit: async (clientHandle, groupIdBase64) =>
    nativeOpenMls().mergePendingCommit(clientHandle, groupIdBase64),
  processWelcome: async (clientHandle, welcomeBase64) =>
    nativeOpenMls().processWelcome(clientHandle, welcomeBase64),
  encrypt: async (clientHandle, groupIdBase64, plaintextBase64) =>
    nativeOpenMls().encrypt(clientHandle, groupIdBase64, plaintextBase64),
  decrypt: async (clientHandle, groupIdBase64, messageBase64) =>
    nativeOpenMls().decrypt(clientHandle, groupIdBase64, messageBase64),
  selfUpdate: async (clientHandle, groupIdBase64) =>
    nativeOpenMls().selfUpdate(clientHandle, groupIdBase64),
  processCommit: async (clientHandle, groupIdBase64, messageBase64) =>
    nativeOpenMls().processCommit(clientHandle, groupIdBase64, messageBase64),
  groupInfo: async (clientHandle, groupIdBase64) =>
    nativeOpenMls().groupInfo(clientHandle, groupIdBase64),
  suspendClient: async clientHandle =>
    nativeOpenMls().suspendClient(clientHandle),
  restoreClient: async snapshotHandle =>
    nativeOpenMls().restoreClient(snapshotHandle),
  releaseClient: async clientHandle =>
    nativeOpenMls().releaseClient(clientHandle),
  discardSnapshot: async snapshotHandle =>
    nativeOpenMls().discardSnapshot(snapshotHandle),
  runSelfTest: async () => {
    if (!__DEV__) {
      throw new OpenMlsBridgeError(
        'DEVELOPMENT_ONLY',
        'The OpenMLS bridge self-test is available only in development builds.',
      );
    }
    const nativeModule = nativeOpenMls();
    if (typeof nativeModule.runSelfTest !== 'function') {
      throw new OpenMlsBridgeError(
        'NATIVE_MODULE_INCOMPATIBLE',
        'This Android build does not include the OpenMLS bridge self-test.',
      );
    }
    return nativeModule.runSelfTest();
  },
};
