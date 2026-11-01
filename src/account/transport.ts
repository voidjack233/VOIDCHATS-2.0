import {AccountError, type AccountFetcher} from './client';

/** React Native's text XHR preserves the native failure that fetch discards. */
function connectionFailure(reason: unknown): AccountError {
  const message = typeof reason === 'string' ? reason.slice(0, 4096) : '';
  if (/UnknownHostException|Unable to resolve host|Name or service not known|No address associated|ENOTFOUND|getaddrinfo/i.test(message)) {
    return new AccountError('network_dns');
  }
  if (/SSL|TLS|certificate|CertPath|Trust anchor|HandshakeException|hostname .*not verified/i.test(message)) {
    return new AccountError('network_tls');
  }
  if (/timed?\s*out|timeout|ETIMEDOUT/i.test(message)) { return new AccountError('network_timeout'); }
  if (/ENETUNREACH|EHOSTUNREACH|Network is unreachable|No route to host|ENETDOWN|network is down/i.test(message)) {
    return new AccountError('network_unavailable');
  }
  if (/ConnectException|ECONNREFUSED|connection refused|connection reset|ECONNRESET|failed to connect|unexpected end of stream/i.test(message)) {
    return new AccountError('network_refused');
  }
  return new AccountError('network');
}

export function createAccountTransport(requestFactory: () => XMLHttpRequest = () => new XMLHttpRequest()): AccountFetcher {
  return (url, options) => new Promise((resolve, reject) => {
    let xhr: XMLHttpRequest | undefined;
    let finished = false;
    const signal = options.signal;
    const cleanup = () => {
      signal?.removeEventListener('abort', abort);
      if (xhr) { xhr.onload = null; xhr.onerror = null; xhr.ontimeout = null; xhr.onabort = null; }
    };
    const fail = (failure: AccountError) => {
      if (finished) { return; }
      finished = true;
      cleanup();
      reject(failure);
    };
    const abort = () => {
      fail(new AccountError('network_timeout'));
      xhr?.abort();
    };
    try {
      xhr = requestFactory();
      const request = xhr;
      if (signal?.aborted) { fail(new AccountError('network_timeout')); return; }
      xhr.open(options.method ?? 'POST', url, true);
      xhr.responseType = 'text';
      xhr.withCredentials = false;
      xhr.timeout = 20000;
      const headers = options.headers;
      if (Array.isArray(headers)) {
        headers.forEach(([name, value]) => request.setRequestHeader(name, value));
      } else if (headers && 'forEach' in headers && typeof headers.forEach === 'function') {
        headers.forEach((value, name) => request.setRequestHeader(name, value));
      } else {
        Object.entries(headers ?? {}).forEach(([name, value]) => request.setRequestHeader(name, value));
      }
      xhr.onerror = () => {
        let reason = '';
        try { reason = request.responseText; } catch { /* Unavailable native detail stays generic. */ }
        fail(connectionFailure(reason));
      };
      xhr.ontimeout = () => fail(new AccountError('network_timeout'));
      xhr.onabort = () => fail(new AccountError('network_timeout'));
      xhr.onload = () => {
        if (finished) { return; }
        try {
          if (request.status === 0) { fail(new AccountError('network')); return; }
          const body = request.responseText;
          const status = request.status;
          const retryAfter = request.getResponseHeader('Retry-After');
          finished = true;
          cleanup();
          resolve({status, headers: {get: name => name.toLowerCase() === 'retry-after' ? retryAfter : null}, text: async () => body});
        } catch { fail(new AccountError('network')); }
      };
      signal?.addEventListener('abort', abort);
      xhr.send(typeof options.body === 'string' ? options.body : null);
    } catch (error) {
      fail(connectionFailure(error instanceof Error ? error.message : ''));
    }
  });
}
