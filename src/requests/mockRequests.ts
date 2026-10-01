export interface MessageRequest {
  id: string;
  name: string;
  voidId: string;
  preview: string;
  receivedAtLabel: string;
}

/** Temporary requests for the UI; no request service is connected yet. */
export const mockRequests: MessageRequest[] = [
  {
    id: 'aria',
    name: 'Aria Kim',
    voidId: 'aria.void',
    preview: 'Hi! Mira mentioned you might be interested in the project.',
    receivedAtLabel: 'Today',
  },
];
