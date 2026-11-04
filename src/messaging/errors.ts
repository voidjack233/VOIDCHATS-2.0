const messages: Record<string, string> = {
  not_registered: 'Initialize your VOID account to find people and exchange messages.',
  native_unavailable: 'Install the updated VOID Android app to enable encrypted messaging.',
  invalid_input: 'Check the VOID-ID or message and try again.',
  invalid_frame: 'The messaging request was rejected. Update VOID and try again.',
  invalid_response: 'VOID returned an incompatible messaging response. Your local data is preserved.',
  invalid_session: 'Could not authenticate your saved VOID identity. Check your connection and try again.',
  authentication_failed: 'The saved identity could not authenticate. Your keys are preserved.',
  unauthorized: 'Reconnect to authenticate your saved VOID identity.',
  not_found: 'That VOID-ID is not registered.',
  duplicate_event: 'Send status is unknown because the server already has this message ID. The saved ciphertext is preserved.',
  queue_full: 'The recipient’s temporary message storage is full. Try again later.',
  rate_limited: 'Too many messaging requests. Wait a moment before trying again.',
  overloaded: 'VOID is busy. Try again shortly.',
  unavailable: 'Could not connect to VOID. Check your connection.',
  peer_offline: 'That device is currently unavailable.',
  deadline_exceeded: 'VOID did not respond in time. Check your connection.',
  send_interrupted: 'Sending was interrupted. Your message is saved.',
  secure_state: 'A message could not be verified. Sending is paused for this chat.',
  prekeys_unavailable: 'Secure messaging is currently unavailable. Try again later.',
  prekeys_conflict: 'The published key data conflicts with an earlier operation. Your saved keys and messages are preserved.',
  prekeys_invalid: 'The public key data was rejected. Your saved keys and messages are preserved.',
  prekeys_capacity: 'The messaging service has reached a limit. Try again later.',
  prekey_claim_expired: 'The saved public key claim has expired. Your message remains saved.',
  unsupported_message: 'This message format is not supported by VOID 2.0. Your existing data is preserved.',
  identity_changed: 'This person’s identity has changed. Sending is paused for this chat.',
  storage: 'Messaging could not update encrypted local storage. Your existing data was preserved.',
  operation_failed: 'Could not complete the messaging operation. Try again.',
};
export class MessagingError extends Error {
  readonly code: string;
  readonly diagnosticCode: string | null;
  constructor(code: string, diagnosticCode: string | null = null) {
    super(messages[code] ?? messages.operation_failed); this.name = 'MessagingError';
    this.code = Object.prototype.hasOwnProperty.call(messages, code) ? code : 'operation_failed';
    this.diagnosticCode = diagnosticCode && /^[A-Z][A-Z0-9_]{0,95}$/.test(diagnosticCode) ? diagnosticCode : null;
  }
}
export function safeMessagingError(value: unknown): MessagingError {
  if (value instanceof MessagingError) { return value; }
  const code = value && typeof value === 'object' && 'code' in value ? value.code : undefined;
  if (typeof code === 'string') {
    const normalized = code.toLowerCase();
    if (Object.prototype.hasOwnProperty.call(messages, normalized)) { return new MessagingError(normalized, code); }
    if (['NATIVE_MODULE_UNAVAILABLE', 'NATIVE_MODULE_INCOMPATIBLE', 'UNSUPPORTED_PLATFORM'].includes(code)) { return new MessagingError('native_unavailable'); }
    if (['PEER_KEY_CHANGED', 'SIGNAL_IDENTITY_CHANGED', 'SIGNAL_UNTRUSTED_IDENTITY'].includes(code)) { return new MessagingError('identity_changed', code); }
    if (code === 'SIGNAL_PREKEYS_UNAVAILABLE') { return new MessagingError('prekeys_unavailable', code); }
    if (code === 'SIGNAL_UNSUPPORTED_MESSAGE') { return new MessagingError('unsupported_message', code); }
    if (/^SIGNAL_.*(?:STORAGE|IDENTITY_MISSING|ENGINE_UNAVAILABLE)/.test(code)) { return new MessagingError('storage', code); }
    if (/^SIGNAL_/.test(code)) { return new MessagingError('secure_state', code); }
    if (/^MLS_|SECURE_|KEY_MISMATCH|INVALID_PROOF|INVALID_MESSAGE/.test(code)) { return new MessagingError('secure_state', code); }
    if (code === 'INVALID_INPUT') { return new MessagingError('invalid_input'); }
    if (/STORAGE|DATABASE|CORRUPT|LOCKED|KEY_UNAVAILABLE|ENCRYPTION_UNAVAILABLE/.test(code)) { return new MessagingError('storage'); }
    if (Object.prototype.hasOwnProperty.call(messages, code)) { return new MessagingError(code); }
  }
  return new MessagingError('operation_failed');
}
export function retryable(error: MessagingError): boolean { return ['unavailable', 'overloaded', 'queue_full', 'rate_limited', 'deadline_exceeded', 'unauthorized'].includes(error.code); }
