/** Public MLS protocol artifacts use standard base64 across the native bridge. */
export type MlsBase64 = string;

declare const clientHandleBrand: unique symbol;
declare const snapshotHandleBrand: unique symbol;

/** Opaque reference to a Rust client retained by the native module. */
export type MlsClientHandle = string & {
  readonly [clientHandleBrand]: true;
};

/** Single-use reference to secret state retained only in native memory. */
export type MlsSnapshotHandle = string & {
  readonly [snapshotHandleBrand]: true;
};

export interface MlsEngineInfo {
  openmlsVersion: string;
  ciphersuite: string;
  protocolVersion: string;
  durablePersistence: false;
}

export interface MlsClientInfo {
  clientHandle: MlsClientHandle;
  credentialIdentityBase64: MlsBase64;
  signaturePublicKeyBase64: MlsBase64;
}

export interface MlsGroupInfo {
  groupIdBase64: MlsBase64;
  /** Decimal u64 string; JavaScript numbers cannot represent every MLS epoch. */
  epoch: string;
  memberCount: number;
}

export interface MlsMemberAddition {
  commitBase64: MlsBase64;
  welcomeBase64: MlsBase64;
}

export interface MlsDecryptedMessage {
  plaintextBase64: MlsBase64;
  senderIdentityBase64: MlsBase64;
  senderSignaturePublicKeyBase64: MlsBase64;
  epoch: string;
}

export interface MlsSelfTestReport {
  openmlsVersion: string;
  aliceToBob: boolean;
  bobToAlice: boolean;
  tamperRejected: boolean;
  commitProcessed: boolean;
  snapshotRestored: boolean;
}

/**
 * Cryptographic operations execute in Rust. This interface exposes no signing
 * private keys, group secrets, or serialized snapshots to JavaScript.
 */
export interface OpenMls {
  initialize(): Promise<MlsEngineInfo>;
  createClient(credentialIdentityBase64: MlsBase64): Promise<MlsClientInfo>;
  generateKeyPackage(clientHandle: MlsClientHandle): Promise<MlsBase64>;
  createGroup(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
  ): Promise<MlsGroupInfo>;
  addMember(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
    keyPackageBase64: MlsBase64,
  ): Promise<MlsMemberAddition>;
  mergePendingCommit(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
  ): Promise<MlsGroupInfo>;
  processWelcome(
    clientHandle: MlsClientHandle,
    welcomeBase64: MlsBase64,
  ): Promise<MlsGroupInfo>;
  encrypt(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
    plaintextBase64: MlsBase64,
  ): Promise<MlsBase64>;
  decrypt(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
    messageBase64: MlsBase64,
  ): Promise<MlsDecryptedMessage>;
  selfUpdate(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
  ): Promise<MlsBase64>;
  processCommit(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
    messageBase64: MlsBase64,
  ): Promise<MlsGroupInfo>;
  groupInfo(
    clientHandle: MlsClientHandle,
    groupIdBase64: MlsBase64,
  ): Promise<MlsGroupInfo>;
  suspendClient(clientHandle: MlsClientHandle): Promise<MlsSnapshotHandle>;
  restoreClient(snapshotHandle: MlsSnapshotHandle): Promise<MlsClientInfo>;
  releaseClient(clientHandle: MlsClientHandle): Promise<void>;
  discardSnapshot(snapshotHandle: MlsSnapshotHandle): Promise<void>;
  /** Available only in a development build; creates isolated Rust test clients. */
  runSelfTest(): Promise<MlsSelfTestReport>;
}

export type OpenMlsBridgeErrorCode =
  | 'UNSUPPORTED_PLATFORM'
  | 'NATIVE_MODULE_UNAVAILABLE'
  | 'NATIVE_MODULE_INCOMPATIBLE'
  | 'DEVELOPMENT_ONLY';

export type OpenMlsAvailability =
  | {available: true; platform: 'android'}
  | {
      available: false;
      code: OpenMlsBridgeErrorCode;
      message: string;
    };
