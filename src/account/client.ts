import {decodeBase64, isChallengeToken, isTimestamp, isVoidId} from './validation';

export type AccountStage = 'idle' | 'preparing' | 'challenge' | 'signing' | 'registering' | 'saving' | 'ready' | 'error';
export interface AccountRegistration {void_id: string; created_at: string}
export interface RegistrationChallenge {challenge_id: string; message: string; expires_at: string}
export interface AccountHttpResponse {
  status: number;
  headers: {get(name: string): string | null};
  text(): Promise<string>;
}
export type AccountFetcher = (url: string, options: RequestInit) => Promise<AccountHttpResponse>;

const messages = {
  invalid_configuration: 'The account server address is unavailable. Update the app configuration.',
  network: 'Could not reach VOID. Check your connection and try again. Your device identity is saved.',
  network_dns: 'Your phone could not find api.voidchats.xyz. Try mobile data or check Private DNS and VPN settings. Your identity is saved.',
  network_tls: 'Your phone could not verify VOID’s HTTPS certificate. Check automatic date and time, and any VPN or HTTPS filtering. Your identity is saved.',
  network_timeout: 'VOID did not respond within 20 seconds. Try again or switch between Wi-Fi and mobile data. Your identity is saved.',
  network_refused: 'The connection to VOID was refused or interrupted. Try another network. Your identity is saved.',
  network_unavailable: 'Your phone has no network route to VOID. Check Wi-Fi, mobile data and VPN settings. Your identity is saved.',
  invalid_response: 'VOID returned an unexpected response. Your device identity is saved. Try again.',
  invalid_input: 'VOID could not accept these account details. Try again with the saved device identity.',
  invalid_proof: 'The setup challenge expired or was rejected. Try again to request a fresh challenge.',
  key_registered: 'This device key is already registered. Keep this app’s local data. Account recovery is not available yet.',
  rate_limited: 'Too many setup attempts. Please wait before trying again.',
  capacity_reached: 'VOID is handling too many setup requests. Please wait and try again.',
  unavailable: 'VOID is temporarily unavailable. Your device identity is saved. Try again later.',
  save_failed: 'Your account was created, but could not be saved on this device. Try again to finish saving it.',
};

export class AccountError extends Error {
  endpoint: string | null = null;
  statusCode: number | null = null;
  constructor(readonly code: keyof typeof messages, readonly retryAfterSeconds: number | null = null) {
    super(messages[code]); this.name = 'AccountError';
  }
}

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) { throw new AccountError('invalid_response'); }
  return value as Record<string, unknown>;
}

export function normalizeApiUrl(value: string): string {
  const url = value.replace(/\/+$/, '');
  if (!/^https:\/\/[A-Za-z0-9.-]+(?::[0-9]+)?(?:\/[A-Za-z0-9._~%-]+)*$/.test(url)) {
    throw new AccountError('invalid_configuration');
  }
  return url;
}

function retryAfter(value: string | null): number | null {
  if (!value) { return null; }
  const seconds = /^\d+$/.test(value) ? Number(value) : Math.ceil((Date.parse(value) - Date.now()) / 1000);
  return Number.isSafeInteger(seconds) && seconds > 0 ? seconds : null;
}

/** No automatic POST retries: a disconnected account submission may have committed. */
export function createAccountClient(apiUrl: string, fetcher: AccountFetcher = fetch, timeoutMs = 20000) {
  const base = normalizeApiUrl(apiUrl);
  const post = async (path: string, payload: object): Promise<unknown> => {
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), timeoutMs);
    try {
      const response = await fetcher(base + path, {
        method: 'POST', headers: {'Content-Type': 'application/json', Accept: 'application/json', 'User-Agent': 'VOIDCHATS/2.0'},
        body: JSON.stringify(payload), signal: abort.signal, credentials: 'omit',
      });
      const body = await response.text();
      if (body.length > 16384) { throw new AccountError('invalid_response'); }
      let decoded: unknown;
      try { decoded = JSON.parse(body) as unknown; } catch {
        const failure = new AccountError(response.status >= 500 ? 'unavailable' : 'invalid_response');
        failure.statusCode = response.status;
        throw failure;
      }
      if (response.status !== 201) {
        const error = object(object(decoded).error);
        const known = ['invalid_input', 'invalid_proof', 'key_registered', 'rate_limited', 'capacity_reached'];
        const code = typeof error.code === 'string' && known.includes(error.code) ? error.code as keyof typeof messages : 'unavailable';
        const failure = new AccountError(code, retryAfter(response.headers.get('Retry-After')));
        failure.statusCode = response.status;
        throw failure;
      }
      return decoded;
    } catch (error) {
      const failure = error instanceof AccountError ? error : new AccountError(abort.signal.aborted ? 'network_timeout' : 'network');
      failure.endpoint = base + path;
      throw failure;
    } finally { clearTimeout(timer); }
  };
  return {
    base,
    challenge: async (publicKey: string): Promise<RegistrationChallenge> => {
      try { if (decodeBase64(publicKey).length !== 32) { throw new Error(); } }
      catch { throw new AccountError('invalid_response'); }
      const row = object(await post('/challenges', {auth_public_key: publicKey}));
      if (!isChallengeToken(row.challenge_id) || !isTimestamp(row.expires_at) ||
          Date.parse(row.expires_at) <= Date.now() || Date.parse(row.expires_at) > Date.now() + 10 * 60 * 1000) { throw new AccountError('invalid_response'); }
      let bytes: number[];
      try { bytes = decodeBase64(row.message); } catch { throw new AccountError('invalid_response'); }
      if (bytes.some(byte => byte > 127)) { throw new AccountError('invalid_response'); }
      const message = String.fromCharCode(...bytes);
      const prefix = `VOID-NOTHINGNESS:register:v1\n${row.challenge_id}\n${row.expires_at}\n${publicKey}\n\n`;
      if (!message.startsWith(prefix) || !message.endsWith('\n') || !isChallengeToken(message.slice(prefix.length, -1))) {
        throw new AccountError('invalid_response');
      }
      return {challenge_id: row.challenge_id, message: row.message as string, expires_at: row.expires_at};
    },
    register: async (challengeId: string, signature: string): Promise<AccountRegistration> => {
      try { if (decodeBase64(signature).length !== 64) { throw new Error(); } }
      catch { throw new AccountError('invalid_response'); }
      const row = object(await post('/accounts', {challenge_id: challengeId, signature}));
      if (!isVoidId(row.void_id) || !isTimestamp(row.created_at) || 'username' in row) { throw new AccountError('invalid_response'); }
      return {void_id: row.void_id, created_at: row.created_at};
    },
  };
}
