const stages = new Set([
  'socket_connect', 'auth_ok', 'socket_disconnect', 'socket_reset', 'socket_transmit', 'reconnect',
  'prekeys_publish', 'prekeys_published', 'prekeys_claim', 'prekeys_claimed', 'prekey_setup_failed',
  'send_submit', 'send_accepted', 'send_local_commit', 'send_failed', 'pending_sends_failed',
  'event_received', 'event_apply', 'event_local_commit', 'event_failed', 'ack_submit', 'ack_ok', 'retry',
] as const);
type DiagnosticStage = typeof stages extends Set<infer T> ? T : never;
interface DiagnosticFields {
  local_void_id?: string | null; recipient_void_id?: string; sender_void_id?: string;
  event_id?: string; request_id?: string; publication_id?: string;
  created_at?: string; expires_at?: string; code?: string; diagnostic_code?: string | null;
  changed?: boolean; command_type?: string;
}
const voidIds = new Set(['local_void_id', 'recipient_void_id', 'sender_void_id']);
const uuids = new Set(['event_id', 'request_id', 'publication_id']);
const times = new Set(['created_at', 'expires_at']);
const codes = new Set(['code', 'diagnostic_code']);
const commands = new Set(['lookup', 'prekeys_publish', 'prekeys_claim', 'relay_online', 'send', 'ack']);

/** Public correlation metadata only. Diagnostic reads and logging cannot fail an operation. */
export function logMessagingDiagnostic(stage: DiagnosticStage, fields: () => DiagnosticFields) {
  try {
    if (!stages.has(stage)) { return; }
    const parts = ['[VoidMessaging]', `stage=${stage}`];
    for (const [key, value] of Object.entries(fields())) {
      if (key === 'changed' && typeof value === 'boolean') { parts.push(`${key}=${value}`); continue; }
      if (typeof value !== 'string') { continue; }
      const valid = voidIds.has(key) ? /^[1-9][0-9]{0,18}$/.test(value)
        : uuids.has(key) ? /^(?!00000000-0000-0000-0000-000000000000$)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value)
        : times.has(key) ? value.length <= 40 && /^[0-9TZ:+.-]+$/.test(value) && Number.isFinite(Date.parse(value))
        : codes.has(key) ? /^[A-Za-z][A-Za-z0-9_]{0,95}$/.test(value)
        : key === 'command_type' ? commands.has(value) : false;
      if (valid) { parts.push(`${key}=${value}`); }
    }
    console.info(parts.join(' '));
  } catch { /* Diagnostics never change transport, storage, or cryptographic outcomes. */ }
}
