/** Public records only. Database keys and serialized MLS state stay native. */
export type StorageStatus =
  | 'opening'
  | 'not_initialized'
  | 'ready'
  | 'locked'
  | 'error';

export type StorageErrorCode =
  | 'UNSUPPORTED_PLATFORM'
  | 'NATIVE_MODULE_UNAVAILABLE'
  | 'NATIVE_MODULE_INCOMPATIBLE'
  | 'NOT_INITIALIZED'
  | 'LOCKED'
  | 'KEY_UNAVAILABLE'
  | 'CORRUPT'
  | 'ENCRYPTION_UNAVAILABLE'
  | 'DATABASE_ERROR'
  | 'INVALID_INPUT'
  | 'NOT_FOUND'
  | 'CONFLICT'
  | 'INVALID_NATIVE_RESPONSE'
  | 'DEVELOPMENT_ONLY'
  | 'OPERATION_FAILED';

export interface LocalIdentity {
  localDeviceId: string;
  clientHandle: string;
  credentialIdentityBase64: string;
  signaturePublicKeyBase64: string;
  /** Local association only; null until a future explicit account binding. */
  accountVoidId: string | null;
  createdAt: number;
}

export interface NativeStorageSnapshot {
  status: 'not_initialized' | 'locked' | 'open' | 'error';
  schemaVersion: number | null;
  identity: LocalIdentity | null;
  errorCode: string | null;
}

export interface MessageCursor {
  createdAt: number;
  id: string;
}

export interface MessagePreview {
  id: string;
  body: string | null;
  createdAt: number;
  deletedAt: number | null;
}

export interface ConversationRecord {
  id: string;
  kind: 'direct' | 'group';
  title: string;
  peerVoidId: string | null;
  mlsGroupId: string | null;
  createdAt: number;
  updatedAt: number;
  unreadCount: number;
  lastMessage: MessagePreview | null;
  members: ConversationMember[];
}

export interface ConversationCursor {
  updatedAt: number;
  id: string;
}

export interface ConversationPage {
  /** Newest activity first, ordered by (updatedAt, id). */
  items: ConversationRecord[];
  nextCursor: ConversationCursor | null;
}

export interface ConversationMember {
  voidId: string;
  displayName: string;
  role: 'member' | 'admin';
  joinedAt: number;
  leftAt: number | null;
}

export interface ConversationInput {
  id: string;
  kind: 'direct' | 'group';
  title: string;
  peerVoidId: string | null;
  mlsGroupId: string | null;
  createdAt: number;
  updatedAt: number;
  members?: ConversationMember[];
}

export type DeliveryState =
  | 'received'
  | 'pending'
  | 'sent'
  | 'delivered'
  | 'read'
  | 'failed';

export interface MessageReaction {
  reactorVoidId: string;
  emoji: string;
  createdAt: number;
}

export interface MessageReceipt {
  recipientVoidId: string;
  state: 'delivered' | 'read';
  recordedAt: number;
}

export interface MessageRecord {
  id: string;
  conversationId: string;
  senderVoidId: string;
  body: string | null;
  direction: 'incoming' | 'outgoing';
  deliveryState: DeliveryState;
  createdAt: number;
  editedAt: number | null;
  deletedAt: number | null;
  readAt: number | null;
  reactions: MessageReaction[];
  receipts: MessageReceipt[];
}

/** Inbound transport/decryption or outbound queue input; this sends nothing. */
export interface MessageInput {
  id: string;
  conversationId: string;
  senderVoidId: string;
  body: string;
  direction: 'incoming' | 'outgoing';
  deliveryState: 'received' | 'pending';
  createdAt: number;
}

export interface MessagePage {
  /** Newest first, ordered by (createdAt, id). */
  items: MessageRecord[];
  nextCursor: MessageCursor | null;
}

export interface ContactRecord {
  id: string;
  voidId: string;
  displayName: string;
  identityPublicKey: string;
  /** Public-key storage does not verify identity ownership. */
  trustState: 'unverified' | 'changed';
  createdAt: number;
  updatedAt: number;
}

export interface ContactInput {
  id: string;
  voidId: string;
  displayName: string;
  identityPublicKey: string;
  createdAt: number;
  updatedAt: number;
}

export type RequestStatus = 'pending' | 'accepted' | 'declined' | 'blocked';

export interface MessageRequestRecord {
  id: string;
  conversationId: string | null;
  fromVoidId: string;
  displayName: string;
  identityPublicKey: string;
  preview: string;
  status: RequestStatus;
  createdAt: number;
  updatedAt: number;
}

export interface MessageRequestInput {
  id: string;
  conversationId: string | null;
  fromVoidId: string;
  displayName: string;
  identityPublicKey: string;
  preview: string;
  createdAt: number;
}

export type StorageOperation =
  | 'conversations.list'
  | 'conversations.page'
  | 'conversations.get'
  | 'conversations.save'
  | 'conversations.markRead'
  | 'messages.page'
  | 'messages.get'
  | 'messages.save'
  | 'messages.edit'
  | 'messages.delete'
  | 'messages.setDeliveryState'
  | 'messages.setReaction'
  | 'messages.setReceipt'
  | 'contacts.list'
  | 'contacts.get'
  | 'contacts.save'
  | 'contacts.remove'
  | 'requests.list'
  | 'requests.save'
  | 'requests.setStatus';

/** No SQL, arbitrary table access, encryption keys, or private MLS state. */
export interface NativeStorageModule {
  getStatus(): Promise<string>;
  open(): Promise<string>;
  initializeDevelopment(): Promise<string>;
  close(): Promise<string>;
  removeLocalData(): Promise<string>;
  getLocalIdentity(): Promise<string>;
  execute(operation: StorageOperation, payloadJSON: string): Promise<string>;
}
