import React, {createContext, useCallback, useContext, useMemo, useRef, useState} from 'react';
import {API_URL} from '../config/api';
import {useStorage} from '../storage';
import {sanitizeStorageError} from '../storage/bridge';
import {AccountError, createAccountClient, type AccountStage} from './client';
import {AccountInitializer} from './initialization';
import {createAccountTransport} from './transport';

interface AccountContextValue {
  status: AccountStage;
  error: string | null;
  connectionDetails: string | null;
  username: string | null;
  canPreview: boolean;
  initialize(): Promise<void>;
}
const Context = createContext<AccountContextValue | null>(null);

export function AccountProvider({children}: {children: React.ReactNode}): React.JSX.Element {
  const storage = useStorage();
  const current = useRef(storage);
  current.current = storage;
  const [status, setStatus] = useState<AccountStage>('idle');
  const [error, setError] = useState<string | null>(null);
  const [connectionDetails, setConnectionDetails] = useState<string | null>(null);
  const [retryUntil, setRetryUntil] = useState(0);
  const initializer = useMemo(() => new AccountInitializer({
    client: createAccountClient(API_URL, createAccountTransport()),
    getIdentity: () => current.current.identity,
    prepare: url => current.current.prepareAccountRegistration(url),
    sign: message => current.current.signAccountChallenge(message),
    complete: account => current.current.completeAccountRegistration(account),
    onStage: setStatus,
  }), []);
  const initialize = useCallback(async () => {
    if (Date.now() < retryUntil) {
      setError(`Please wait ${Math.ceil((retryUntil - Date.now()) / 1000)} seconds before trying again.`);
      return;
    }
    setError(null);
    setConnectionDetails(null);
    try { await initializer.initialize(); }
    catch (value) {
      const failure = value instanceof AccountError ? value : sanitizeStorageError(value);
      setError(failure.message);
      setStatus('error');
      if (value instanceof AccountError && value.endpoint) {
        setConnectionDetails(`${value.code.toUpperCase()}${value.statusCode === null ? '' : ` · HTTP ${value.statusCode}`}\n${value.endpoint}`);
      }
      if (value instanceof AccountError && value.retryAfterSeconds !== null) {
        setRetryUntil(Date.now() + value.retryAfterSeconds * 1000);
        setError(`${value.message} Try again in ${value.retryAfterSeconds} seconds.`);
      }
    }
  }, [initializer, retryUntil]);
  const registered = storage.status === 'ready' && Boolean(storage.identity?.accountVoidId);
  return <Context.Provider value={{
    status: registered ? 'ready' : status === 'ready' ? 'idle' : status,
    error,
    connectionDetails,
    username: storage.identity?.accountUsername ?? null,
    canPreview: !initializer.hasPendingRegistration && !registered,
    initialize,
  }}>{children}</Context.Provider>;
}

export function useAccount(): AccountContextValue {
  const value = useContext(Context);
  if (!value) { throw new Error('AccountProvider is required.'); }
  return value;
}
