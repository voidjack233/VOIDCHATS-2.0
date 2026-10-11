export interface BackendConfig {
  readonly baseURL: string;
}

let developmentConfig: BackendConfig | null = null;

// Keep this independent of mobile environment files: server credentials never
// belong in a bundle. This milestone supports an IPv4 address or DNS hostname.
export function createBackendConfig(input: string): BackendConfig {
  const match = /^(https?):\/\/([a-z0-9.-]+)(?::([0-9]{1,5}))?\/?$/i.exec(input.trim());
  if (!match) {
    throw new Error('Enter an HTTP or HTTPS server address without a path, credentials, query, or fragment.');
  }

  const protocol = match[1].toLowerCase();
  const hostname = match[2].toLowerCase();
  const port = match[3];
  if (protocol === 'http' && !__DEV__) {
    throw new Error('Release builds require an HTTPS backend.');
  }
  if (port && (Number(port) < 1 || Number(port) > 65535)) {
    throw new Error('The server port must be between 1 and 65535.');
  }

  const labels = hostname.replace(/\.$/, '').split('.');
  if (hostname.length > 253 || labels.some(label => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))) {
    throw new Error('Use a valid IPv4 address or DNS hostname.');
  }
  if (/^[0-9.]+$/.test(hostname) && (labels.length !== 4 || labels.some(label => Number(label) > 255))) {
    throw new Error('Use a valid IPv4 address.');
  }

  return {baseURL: `${protocol}://${hostname}${port ? `:${port}` : ''}`};
}

export function getDevelopmentBackendConfig(): BackendConfig | null {
  return __DEV__ ? developmentConfig : null;
}

// Development selections live only in this JavaScript session, never in Git,
// native preferences, or a release configuration.
export function setDevelopmentBackendBaseURL(input: string): BackendConfig {
  if (!__DEV__) {
    throw new Error('The backend selector is available only in development builds.');
  }
  developmentConfig = createBackendConfig(input);
  return developmentConfig;
}
