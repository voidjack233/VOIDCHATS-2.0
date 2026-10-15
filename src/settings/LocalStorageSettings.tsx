import React, {useEffect, useRef, useState} from 'react';
import {Alert, StyleSheet, View} from 'react-native';
import {useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button, Surface} from '../ui';

export function LocalStorageSettings() {
  const theme = useTheme();
  const storage = useStorage();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  async function update(operation: () => Promise<void>) {
    if (busy) { return; }
    setBusy(true);
    setError(null);
    try {
      await operation();
    } catch {
      if (mounted.current) { setError('Could not complete the storage operation. Check the local storage status before continuing.'); }
    } finally {
      if (mounted.current) { setBusy(false); }
    }
  }

  function removeData() {
    Alert.alert(
      'Delete local data?',
      'This permanently deletes this device’s conversations, messages, contacts, requests, and MLS identity. It cannot be undone.',
      [
        {text: 'Cancel', style: 'cancel'},
        {text: 'Delete local data', style: 'destructive', onPress: () => { update(storage.removeLocalData); }},
      ],
    );
  }

  const title = storage.status === 'ready' ? 'Local storage open'
    : storage.status === 'opening' ? 'Opening local storage'
    : storage.status === 'not_initialized' ? 'Local storage not set up'
    : storage.status === 'locked' ? 'Local storage closed' : 'Local storage unavailable';

  return (
    <View style={{marginTop: theme.spacing.xxl}}>
      <AppText variant="caption" color="textMuted" style={[styles.label, {marginBottom: theme.spacing.sm}]}>LOCAL STORAGE</AppText>
      <Surface bordered padded>
        <AppText variant="heading">{title}</AppText>
        <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>
          {storage.message ?? (storage.status === 'ready'
            ? 'Conversations, messages, contacts, and requests are stored encrypted on this device.'
            : 'Existing local identities are restored when storage opens.')}
        </AppText>
        {storage.identity && !storage.identity.accountVoidId ? (
          <AppText variant="caption" color="warning" style={{marginTop: theme.spacing.sm}}>This local identity is not a registered VOID account.</AppText>
        ) : null}
        {error ? <AppText variant="caption" color="danger" style={{marginTop: theme.spacing.sm}} accessibilityLiveRegion="polite">{error}</AppText> : null}
        {storage.status === 'not_initialized' && __DEV__ ? (
          <>
            <AppText variant="caption" color="textMuted" style={{marginTop: theme.spacing.lg}}>Development setup creates a new local device identity. It does not register an account or populate sample messages.</AppText>
            <Button title="Set up local storage" loading={busy} onPress={() => { update(storage.createDevelopmentStorage); }} style={{marginTop: theme.spacing.md}} />
          </>
        ) : null}
        {storage.status === 'locked' || storage.status === 'error' ? (
          <Button title="Open existing storage" variant="secondary" loading={busy} onPress={() => { update(storage.openExisting); }} style={{marginTop: theme.spacing.lg}} />
        ) : null}
        {storage.status === 'ready' ? (
          <Button title="Close local storage" variant="secondary" loading={busy} onPress={() => { update(storage.closeStore); }} style={{marginTop: theme.spacing.lg}} />
        ) : null}
        {storage.status !== 'opening' && storage.status !== 'not_initialized' ? (
          <Button title="Delete local data" variant="danger" disabled={busy} onPress={removeData} style={{marginTop: theme.spacing.md}} />
        ) : null}
      </Surface>
    </View>
  );
}

const styles = StyleSheet.create({label: {letterSpacing: 1}});
