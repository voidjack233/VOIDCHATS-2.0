import React, {createContext, useContext, useEffect, useMemo, useRef, useSyncExternalStore} from 'react';
import {AppState} from 'react-native';
import {API_URL} from '../config/api';
import {useStorage} from '../storage';
import {nativeMessaging} from './bridge';
import type {Socket} from './client';
import {MessagingController} from './controller';

const Context = createContext<MessagingController | null>(null);
export function MessagingProvider({children}: {children: React.ReactNode}) {
  const storage = useStorage();
  const current = useRef(storage); current.current = storage;
  const controller = useMemo(() => new MessagingController({apiUrl: API_URL, driver: nativeMessaging,
    identity: () => current.current.status === 'ready' ? current.current.identity : null,
    committed: scope => current.current.notifyCommitted(scope),
    socketFactory: url => new WebSocket(url) as unknown as Socket,
  }), []);
  const scope = storage.status === 'ready' && storage.identity?.accountVoidId ? storage.identity.localDeviceId + ':' + storage.identity.accountVoidId : null;
  useEffect(() => {
    if (scope && AppState.currentState !== 'background') { controller.start(); }
    const subscription = AppState.addEventListener('change', state => {
      if (state === 'active' && scope) { controller.start(); }
      else if (state === 'background') { controller.stop(); }
    });
    return () => { subscription.remove(); controller.stop(); };
  }, [controller, scope]);
  return <Context.Provider value={controller}>{children}</Context.Provider>;
}
export function useMessaging() {
  const controller = useContext(Context);
  if (!controller) { throw new Error('MessagingProvider is required.'); }
  return {...useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot),
    refresh: controller.refresh, reconnect: controller.reconnect, lookup: controller.lookup, openConversation: controller.openConversation,
    blockPeer: controller.blockPeer, logSendDiagnostics: controller.logSendDiagnostics,
    sendMessage: controller.sendMessage, retryOperation: controller.retryOperation, retryMessage: controller.retryMessage};
}
