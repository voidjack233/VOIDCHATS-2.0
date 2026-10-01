export interface Contact {
  id: string;
  name: string;
  voidId: string;
  note?: string;
}

/** Temporary contacts for the discovery UI. */
export const mockContacts: Contact[] = [
  { id: 'mira', name: 'Mira Chen', voidId: 'mira.void' },
  { id: 'jonah', name: 'Jonah Park', voidId: 'jonah.void' },
  { id: 'nora', name: 'Nora Lee', voidId: 'nora.void' },
];
