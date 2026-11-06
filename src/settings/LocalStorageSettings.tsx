import React, {useEffect, useRef, useState} from 'react';
import {Alert, StyleSheet, View} from 'react-native';
import {useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button, Divider} from '../ui';

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
      storage.identity?.accountVoidId
        ? 'This permanently deletes this device’s conversations, messages, contacts, requests, and identity keys. Your registered account remains on the server. Account recovery is not available, so you will lose access to this account. It cannot be undone.'
        : 'This permanently deletes this device’s conversations, messages, contacts, requests, and MLS identity. It cannot be undone.',
      [
        {text: 'Cancel', style: 'cancel'},
        {text: 'Delete local data', style: 'destructive', onPress: () => { update(storage.removeLocalData); }},
      ],
    );
  }

  const status = storage.status === 'ready' ? 'Open'
    : storage.status === 'opening' ? 'Opening…'
    : storage.status === 'not_initialized' ? 'Not set up'
    : storage.status === 'locked' ? 'Closed' : 'Unavailable';

  return (
    <View style={{marginTop: theme.spacing.xxl}}>
      <AppText variant="caption" color="textMuted" style={[styles.label, {marginBottom: theme.spacing.sm}]}>LOCAL STORAGE</AppText>
      <Divider />
      <View style={{paddingVertical: theme.spacing.lg}}>
        <View style={styles.statusRow}>
          <AppText variant="body" style={styles.flex}>Storage status</AppText>
          <AppText color="textSecondary">{status}</AppText>
        </View>
        <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>
          {storage.message ?? (storage.status === 'ready'
            ? 'Your messages and contacts are encrypted on this device.'
            : 'Your local data is stored on this device.')}
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
          <Button title="Open existing storage" variant="secondary" fullWidth loading={busy} onPress={() => { update(storage.openExisting); }} style={{marginTop: theme.spacing.lg}} />
        ) : null}
        {storage.status === 'ready' ? (
          <Button title="Close local storage" variant="secondary" fullWidth loading={busy} onPress={() => { update(storage.closeStore); }} style={{marginTop: theme.spacing.lg}} />
        ) : null}
        {storage.status !== 'opening' && storage.status !== 'not_initialized' ? (
          <Button title="Delete local data" variant="danger" fullWidth disabled={busy} onPress={removeData} style={{marginTop: theme.spacing.md}} />
        ) : null}
      </View>
      <Divider />
    </View>
  );
}

const styles = StyleSheet.create({label: {letterSpacing: 1}, statusRow: {flexDirection: 'row', alignItems: 'center'}, flex: {flex: 1}});
