export interface ChatMessage {
  id: string;
  conversationId: string;
  body: string;
  timeLabel: string;
  direction: 'incoming' | 'outgoing';
}

/** Temporary transcript content for the chat shell. */
export const mockMessages: ChatMessage[] = [
  {
    id: 'mira-1',
    conversationId: 'mira',
    body: 'I sent over the first draft this morning.',
    timeLabel: '10:28',
    direction: 'incoming',
  },
  {
    id: 'mira-2',
    conversationId: 'mira',
    body: 'Thanks! I’m reading through it now.',
    timeLabel: '10:35',
    direction: 'outgoing',
  },
  {
    id: 'mira-3',
    conversationId: 'mira',
    body: 'The first draft looks good. Let’s talk tomorrow.',
    timeLabel: '10:42',
    direction: 'incoming',
  },
  {
    id: 'studio-1',
    conversationId: 'studio',
    body: 'I added a few references.',
    timeLabel: 'Yesterday',
    direction: 'incoming',
  },
  {
    id: 'jonah-1',
    conversationId: 'jonah',
    body: 'See you there.',
    timeLabel: 'Mon',
    direction: 'incoming',
  },
  {
    id: 'weekend-1',
    conversationId: 'weekend',
    body: 'Saturday works for me.',
    timeLabel: 'Sun',
    direction: 'incoming',
  },
  {
    id: 'aria-1',
    conversationId: 'request-aria',
    body: 'Hi! Mira mentioned you might be interested in the project.',
    timeLabel: 'Today',
    direction: 'incoming',
  },
];
