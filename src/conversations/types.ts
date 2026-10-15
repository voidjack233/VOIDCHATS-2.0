import type {ConversationRecord} from '../storage/types';

/** Display model built from a persisted conversation. */
export interface Conversation {
  id: string;
  name: string;
  kind: 'direct' | 'group';
  voidId?: string;
  preview: string;
  updatedAtLabel: string;
  unreadCount: number;
}

export function conversationFromRecord(record: ConversationRecord): Conversation {
  return {
    id: record.id,
    name: record.title,
    kind: record.kind,
    voidId: record.peerVoidId ?? undefined,
    preview: record.lastMessage?.deletedAt != null ? 'Message deleted' : record.lastMessage?.body ?? '',
    updatedAtLabel: new Date(record.updatedAt).toLocaleDateString(undefined, {month: 'short', day: 'numeric'}),
    unreadCount: record.unreadCount,
  };
}
