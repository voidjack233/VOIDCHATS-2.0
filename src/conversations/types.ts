/** Display model for the conversation list and chat shell. */
export interface Conversation {
  id: string;
  name: string;
  kind: 'direct' | 'group';
  voidId?: string;
  preview: string;
  updatedAtLabel: string;
  unreadCount: number;
}
