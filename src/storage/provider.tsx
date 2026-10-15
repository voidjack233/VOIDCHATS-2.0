import React, {createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore} from 'react';
import {sanitizeStorageError, type StorageDriver} from './bridge';
import {StorageController} from './controller';
import {nativeStorage} from './native';
import {isCurrentScope, type StoredViewScope} from './scope';
import type {ContactRecord, ConversationCursor, ConversationRecord, MessageCursor, MessageRecord, MessageRequestRecord, RequestStatus} from './types';

const Context = createContext<StorageController | null>(null);

export function StorageProvider({children, driver = nativeStorage}: {children: React.ReactNode; driver?: StorageDriver}): React.JSX.Element {
  const controller = useMemo(() => new StorageController(driver), [driver]);
  useEffect(() => {
    // Existing encrypted data may resume; first launch creates no identity.
    controller.openExisting().catch(() => {});
    return () => { controller.closeStore().catch(() => {}); };
  }, [controller]);
  return <Context.Provider value={controller}>{children}</Context.Provider>;
}

export function useStorage() {
  const controller = useContext(Context);
  if (!controller) { throw new Error('StorageProvider is required.'); }
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
  return {...state, repositories: controller.repositories,
    openExisting: controller.openExisting, createDevelopmentStorage: controller.createDevelopmentStorage,
    closeStore: controller.closeStore, removeLocalData: controller.removeLocalData};
}

function useRecords<T>(load: () => Promise<T[]>, collection: string) {
  const storage = useStorage();
  const localDeviceId = storage.identity?.localDeviceId ?? null;
  const [data, setData] = useState<{scope: StoredViewScope | null; items: T[]}>({scope: null, items: []});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshVersion, setRefreshVersion] = useState(0);
  useEffect(() => {
    let current = true;
    setError(null);
    if (storage.status !== 'ready' || localDeviceId === null) { setData({scope: null, items: []}); setLoading(false); return; }
    setLoading(true);
    load().then(result => { if (current) { setData({scope: {localDeviceId, collection}, items: result}); } })
      .catch(value => { if (current) { setError(sanitizeStorageError(value).message); } })
      .finally(() => { if (current) { setLoading(false); } });
    return () => { current = false; };
  }, [load, collection, localDeviceId, storage.status, storage.revision, refreshVersion]);
  const refresh = useCallback(() => setRefreshVersion(value => value + 1), []);
  return {items: storage.status === 'ready' && isCurrentScope(data.scope, localDeviceId, collection) ? data.items : [], loading: storage.status === 'ready' && loading, error, refresh};
}

const conversationCursorKey = (cursor: ConversationCursor) => cursor.updatedAt + ':' + cursor.id;
const messageCursorKey = (cursor: MessageCursor) => cursor.createdAt + ':' + cursor.id;

export function useConversations(limit = 50) {
  const {repositories} = useStorage();
  const load = useCallback((before: ConversationCursor | null, size: number) => repositories.conversations.page(before, size), [repositories]);
  return usePagedRecords<ConversationRecord, ConversationCursor>(load, 'conversations', limit, conversationCursorKey);
}
export function useContacts() {
  const {repositories} = useStorage();
  const load = useCallback(() => repositories.contacts.list(), [repositories]);
  return useRecords<ContactRecord>(load, 'contacts');
}

export function useRequests(status: RequestStatus | null = null) {
  const {repositories} = useStorage();
  const load = useCallback(() => repositories.requests.list(status), [repositories, status]);
  return useRecords<MessageRequestRecord>(load, 'requests:' + (status ?? 'all'));
}

export function useMessages(conversationId: string, limit = 50) {
  const {repositories} = useStorage();
  const load = useCallback((before: MessageCursor | null, size: number) => repositories.messages.page(conversationId, before, size), [repositories, conversationId]);
  return usePagedRecords<MessageRecord, MessageCursor>(load, 'messages:' + conversationId, limit, messageCursorKey);
}

function usePagedRecords<T extends {id: string}, Cursor>(
  load: (before: Cursor | null, limit: number) => Promise<{items: T[]; nextCursor: Cursor | null}>,
  collection: string,
  limit: number,
  cursorKey: (cursor: Cursor) => string,
) {
  const storage = useStorage();
  const localDeviceId = storage.identity?.localDeviceId ?? null;
  const [data, setData] = useState<{scope: StoredViewScope | null; items: T[]; nextCursor: Cursor | null}>({scope: null, items: [], nextCursor: null});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const generation = useRef(0);
  const loadingRef = useRef(false);
  const loaded = useRef<{scope: StoredViewScope | null; count: number}>({scope: null, count: 0});
  useEffect(() => {
    const activeGeneration = generation;
    const current = ++activeGeneration.current;
    setError(null);
    if (storage.status !== 'ready' || localDeviceId === null) {
      setData({scope: null, items: [], nextCursor: null});
      loaded.current = {scope: null, count: 0}; setLoading(false); loadingRef.current = false;
      return;
    }
    const targetCount = isCurrentScope(loaded.current.scope, localDeviceId, collection) ? Math.max(limit, loaded.current.count) : limit;
    loadingRef.current = true; setLoading(true);
    const refreshPages = async () => {
      const items: T[] = [];
      const ids = new Set<string>();
      const cursors = new Set<string>();
      let nextCursor: Cursor | null = null;
      do {
        const page = await load(nextCursor, limit);
        if (generation.current !== current) { return; }
        for (const item of page.items) { if (!ids.has(item.id)) { items.push(item); ids.add(item.id); } }
        nextCursor = page.nextCursor;
        if (nextCursor !== null) {
          const key = cursorKey(nextCursor);
          if (cursors.has(key)) { throw new Error('Repeated native page cursor.'); }
          cursors.add(key);
        }
      } while (nextCursor !== null && items.length < targetCount);
      const scope = {localDeviceId, collection};
      loaded.current = {scope, count: items.length};
      setData({scope, items, nextCursor});
    };
    refreshPages().catch(value => { if (generation.current === current) { setError(sanitizeStorageError(value).message); } })
      .finally(() => { if (generation.current === current) { setLoading(false); loadingRef.current = false; } });
    return () => { activeGeneration.current++; };
  }, [load, cursorKey, storage.status, storage.revision, localDeviceId, collection, limit, refreshVersion]);
  const loadMore = useCallback(async () => {
    if (storage.status !== 'ready' || loadingRef.current || data.nextCursor === null ||
        !isCurrentScope(data.scope, localDeviceId, collection)) { return; }
    const current = generation.current;
    loadingRef.current = true; setLoading(true); setError(null);
    try {
      const page = await load(data.nextCursor, limit);
      if (generation.current === current && localDeviceId !== null) {
        setData(previous => {
          const existing = new Set(previous.items.map(item => item.id));
          const items = [...previous.items, ...page.items.filter(item => !existing.has(item.id))];
          const scope = {localDeviceId, collection};
          loaded.current = {scope, count: items.length};
          return {scope, items, nextCursor: page.nextCursor};
        });
      }
    } catch (value) {
      if (generation.current === current) { setError(sanitizeStorageError(value).message); }
    } finally {
      if (generation.current === current) { setLoading(false); loadingRef.current = false; }
    }
  }, [load, storage.status, localDeviceId, collection, data, limit]);
  const refresh = useCallback(() => setRefreshVersion(value => value + 1), []);
  const visible = storage.status === 'ready' && isCurrentScope(data.scope, localDeviceId, collection);
  return {items: visible ? data.items : [], loading: storage.status === 'ready' && loading,
    error, loadMore, hasMore: visible && data.nextCursor !== null, refresh};
}
