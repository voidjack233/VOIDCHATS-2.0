import {sanitizeStorageError, StorageError, type StorageDriver} from './bridge';
import {decodeContact, decodeConversation, decodeConversationPage, decodeList, decodeMessage, decodeNullable, decodePage, decodeRequest} from './decode';
import type {ContactInput, ConversationCursor, ConversationInput, DeliveryState, MessageCursor, MessageInput, MessageRequestInput, RequestStatus, StorageOperation} from './types';

function identifier(value: string) {
  if (typeof value !== 'string' || !value.trim() || value.includes('\0')) { throw new StorageError('INVALID_INPUT'); }
}
function timestamp(value: number) {
  if (!Number.isSafeInteger(value) || value < 0) { throw new StorageError('INVALID_INPUT'); }
}
function keys(value: object, allowed: readonly string[]) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).some(key => !allowed.includes(key))) { throw new StorageError('INVALID_INPUT'); }
}
function body(value: string) {
  if (typeof value !== 'string' || value.includes('\0')) { throw new StorageError('INVALID_INPUT'); }
}
function cursor(value: MessageCursor) {
  keys(value, ['createdAt', 'id']); identifier(value.id); timestamp(value.createdAt);
}

/** Repository writes persist records; no method sends a message or verifies identity. */
export function createRepositories(
  driver: StorageDriver,
  onMutation: () => void = () => {},
  assertReady: () => void = () => {},
  onFailure: (error: StorageError) => void = () => {},
) {
  const call = async <T>(operation: StorageOperation, payload: object, decode: (value: unknown) => T, mutation = false): Promise<T> => {
    assertReady();
    try {
      const value = await driver.execute(operation, payload);
      const result = decode(value);
      if (mutation) { onMutation(); }
      return result;
    } catch (value) {
      const error = sanitizeStorageError(value);
      onFailure(error);
      throw error;
    }
  };
  const nothing = (value: unknown): void => {
    if (value !== null) { throw new StorageError('INVALID_NATIVE_RESPONSE'); }
  };
  return {
    conversations: {
      list: () => call('conversations.list', {}, value => decodeList(value, decodeConversation)),
      page: (before: ConversationCursor | null = null, limit = 50) => {
        if (!Number.isInteger(limit) || limit < 1 || limit > 100) { throw new StorageError('INVALID_INPUT'); }
        if (before !== null) { keys(before, ['updatedAt', 'id']); identifier(before.id); timestamp(before.updatedAt); }
        return call('conversations.page', {before, limit}, value => decodeConversationPage(value, before, limit));
      },
      get: (id: string) => { identifier(id); return call('conversations.get', {id}, value => decodeNullable(value, decodeConversation)); },
      save: (input: ConversationInput) => {
        keys(input, ['id', 'kind', 'title', 'peerVoidId', 'mlsGroupId', 'createdAt', 'updatedAt', 'members']);
        identifier(input.id); identifier(input.title); timestamp(input.createdAt); timestamp(input.updatedAt);
        if (!['direct', 'group'].includes(input.kind) || input.updatedAt < input.createdAt ||
            (input.kind === 'direct' ? !input.peerVoidId : input.peerVoidId !== null)) { throw new StorageError('INVALID_INPUT'); }
        if (input.peerVoidId !== null) { identifier(input.peerVoidId); }
        if (input.mlsGroupId !== null) { identifier(input.mlsGroupId); }
        for (const member of input.members ?? []) {
          keys(member, ['voidId', 'displayName', 'role', 'joinedAt', 'leftAt']);
          identifier(member.voidId); body(member.displayName); timestamp(member.joinedAt);
          if (!['member', 'admin'].includes(member.role)) { throw new StorageError('INVALID_INPUT'); }
          if (member.leftAt !== null) { timestamp(member.leftAt); if (member.leftAt < member.joinedAt) { throw new StorageError('INVALID_INPUT'); } }
        }
        return call('conversations.save', input, decodeConversation, true);
      },
      markRead: (conversationId: string, through: MessageCursor, readAt = Date.now()) => {
        identifier(conversationId); cursor(through); timestamp(readAt);
        return call('conversations.markRead', {conversationId, through, readAt}, nothing, true);
      },
    },
    messages: {
      page: (conversationId: string, before: MessageCursor | null = null, limit = 50) => {
        identifier(conversationId);
        if (!Number.isInteger(limit) || limit < 1 || limit > 100) { throw new StorageError('INVALID_INPUT'); }
        if (before !== null) { cursor(before); }
        return call('messages.page', {conversationId, before, limit}, decodePage);
      },
      get: (id: string) => { identifier(id); return call('messages.get', {id}, value => decodeNullable(value, decodeMessage)); },
      save: (input: MessageInput) => {
        keys(input, ['id', 'conversationId', 'senderVoidId', 'body', 'direction', 'deliveryState', 'createdAt']);
        identifier(input.id); identifier(input.conversationId); identifier(input.senderVoidId); body(input.body); timestamp(input.createdAt);
        if (!(input.direction === 'incoming' && input.deliveryState === 'received') &&
            !(input.direction === 'outgoing' && input.deliveryState === 'pending')) { throw new StorageError('INVALID_INPUT'); }
        return call('messages.save', input, decodeMessage, true);
      },
      edit: (id: string, nextBody: string, editedAt = Date.now()) => {
        identifier(id); body(nextBody); timestamp(editedAt);
        return call('messages.edit', {id, body: nextBody, editedAt}, decodeMessage, true);
      },
      delete: (id: string, deletedAt = Date.now()) => {
        identifier(id); timestamp(deletedAt);
        return call('messages.delete', {id, deletedAt}, decodeMessage, true);
      },
      setDeliveryState: (id: string, state: Exclude<DeliveryState, 'received'>) => {
        identifier(id);
        if (!['pending', 'sent', 'delivered', 'read', 'failed'].includes(state)) { throw new StorageError('INVALID_INPUT'); }
        return call('messages.setDeliveryState', {id, state}, decodeMessage, true);
      },
      setReaction: (id: string, reactorVoidId: string, emoji: string, active: boolean, createdAt = Date.now()) => {
        identifier(id); identifier(reactorVoidId); identifier(emoji); timestamp(createdAt);
        if (typeof active !== 'boolean') { throw new StorageError('INVALID_INPUT'); }
        return call('messages.setReaction', {id, reactorVoidId, emoji, active, createdAt}, decodeMessage, true);
      },
      setReceipt: (id: string, recipientVoidId: string, state: 'delivered' | 'read', recordedAt = Date.now()) => {
        identifier(id); identifier(recipientVoidId); timestamp(recordedAt);
        if (!['delivered', 'read'].includes(state)) { throw new StorageError('INVALID_INPUT'); }
        return call('messages.setReceipt', {id, recipientVoidId, state, recordedAt}, decodeMessage, true);
      },
    },
    contacts: {
      list: () => call('contacts.list', {}, value => decodeList(value, decodeContact)),
      get: (id: string) => { identifier(id); return call('contacts.get', {id}, value => decodeNullable(value, decodeContact)); },
      save: (input: ContactInput) => {
        keys(input, ['id', 'voidId', 'displayName', 'identityPublicKey', 'createdAt', 'updatedAt']);
        identifier(input.id); identifier(input.voidId); identifier(input.displayName); identifier(input.identityPublicKey);
        timestamp(input.createdAt); timestamp(input.updatedAt);
        if (input.updatedAt < input.createdAt) { throw new StorageError('INVALID_INPUT'); }
        return call('contacts.save', input, decodeContact, true);
      },
      remove: (id: string) => { identifier(id); return call('contacts.remove', {id}, nothing, true); },
    },
    requests: {
      list: (status: RequestStatus | null = null) => {
        if (status !== null && !['pending', 'accepted', 'declined', 'blocked'].includes(status)) { throw new StorageError('INVALID_INPUT'); }
        return call('requests.list', {status}, value => decodeList(value, decodeRequest));
      },
      save: (input: MessageRequestInput) => {
        keys(input, ['id', 'conversationId', 'fromVoidId', 'displayName', 'identityPublicKey', 'preview', 'createdAt']);
        identifier(input.id); identifier(input.fromVoidId); body(input.displayName); identifier(input.identityPublicKey); body(input.preview); timestamp(input.createdAt);
        if (input.conversationId !== null) { identifier(input.conversationId); }
        return call('requests.save', input, decodeRequest, true);
      },
      setStatus: (id: string, status: RequestStatus, updatedAt = Date.now()) => {
        identifier(id); timestamp(updatedAt);
        if (!['pending', 'accepted', 'declined', 'blocked'].includes(status)) { throw new StorageError('INVALID_INPUT'); }
        return call('requests.setStatus', {id, status, updatedAt}, decodeRequest, true);
      },
    },
  };
}
export type StorageRepositories = ReturnType<typeof createRepositories>;

