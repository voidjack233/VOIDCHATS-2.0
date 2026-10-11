import {createBackendConfig} from './config';
import type {BackendConfig} from './config';

export interface BackendHealthResult {
  readonly ok: boolean;
  readonly message: string;
  readonly status?: number;
}

const healthTimeoutMs = 8000;

export async function checkBackendHealth(config: BackendConfig, signal?: AbortSignal): Promise<BackendHealthResult> {
  if (!__DEV__) {
    return {ok: false, message: 'The backend diagnostic is available only in development builds.'};
  }

  // Revalidate even if a caller supplied an arbitrary object as configuration.
  const {baseURL} = createBackendConfig(config.baseURL);
  const controller = new AbortController();
  const abort = () => controller.abort();
  signal?.addEventListener('abort', abort);
  if (signal?.aborted) {
    abort();
  }
  let timedOut = false;
  const timeout = setTimeout(() => {
    timedOut = true;
    abort();
  }, healthTimeoutMs);
  const startedAt = Date.now();

  try {
    const response = await fetch(`${baseURL}/health`, {
      method: 'GET',
      headers: {Accept: 'application/json'},
      credentials: 'omit',
      signal: controller.signal,
    });
    if (response.status !== 200) {
      return {ok: false, status: response.status, message: `Backend returned HTTP ${response.status}. Check the address or tunnel access policy.`};
    }
    if (!response.headers.get('content-type')?.toLowerCase().includes('application/json')) {
      return {ok: false, status: response.status, message: 'The server did not return health JSON. Check the address or tunnel authentication.'};
    }

    const body: unknown = await response.json();
    if (controller.signal.aborted) {
      throw new Error('Request aborted');
    }
    if (typeof body !== 'object' || body === null || !('status' in body) || body.status !== 'ok') {
      return {ok: false, status: response.status, message: 'The server returned an unexpected health response.'};
    }
    return {ok: true, status: response.status, message: `Connected (HTTP 200, ${Date.now() - startedAt} ms).`};
  } catch {
    if (timedOut) {
      return {ok: false, message: 'Timed out after 8 seconds. Check Wi-Fi, the API listener, and the host firewall.'};
    }
    if (signal?.aborted) {
      return {ok: false, message: 'Connection check canceled.'};
    }
    return {ok: false, message: 'Unable to check health. Check the URL, network, TLS certificate, or tunnel access policy.'};
  } finally {
    clearTimeout(timeout);
    signal?.removeEventListener('abort', abort);
  }
}
