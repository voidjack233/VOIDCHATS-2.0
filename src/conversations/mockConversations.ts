import type { Conversation } from './types';

/** Temporary content for developing the UI. Replace with application state later. */
export const mockConversations: Conversation[] = [
  {
    id: 'mira',
    name: 'Mira Chen',
    kind: 'direct',
    voidId: 'mira.void',
    preview: 'The first draft looks good. Let’s talk tomorrow.',
    updatedAtLabel: '10:42',
    unreadCount: 2,
  },
  {
    id: 'studio',
    name: 'Studio notes',
    kind: 'group',
    preview: 'Alex: I added a few references.',
    updatedAtLabel: 'Yesterday',
    unreadCount: 0,
  },
  {
    id: 'jonah',
    name: 'Jonah Park',
    kind: 'direct',
    voidId: 'jonah.void',
    preview: 'See you there.',
    updatedAtLabel: 'Mon',
    unreadCount: 0,
  },
  {
    id: 'weekend',
    name: 'Weekend plans',
    kind: 'group',
    preview: 'Sam: Saturday works for me.',
    updatedAtLabel: 'Sun',
    unreadCount: 0,
  },
];
