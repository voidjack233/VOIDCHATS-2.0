import React, {useEffect, useRef, useState} from 'react';
import {FlatList, StyleSheet, View} from 'react-native';
import {useRequests, useStorage} from '../storage';
import type {RequestStatus} from '../storage/types';
import {useTheme} from '../theme';
import {AppText, Button, Screen, Surface} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export function RequestsScreen() {
  const theme = useTheme();
  const storage = useStorage();
  const {items, loading, error, refresh} = useRequests();
  const [updating, setUpdating] = useState<string | null>(null);
  const [updateError, setUpdateError] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  async function update(id: string, status: RequestStatus) {
    if (updating) { return; }
    setUpdating(id);
    setUpdateError(false);
    try {
      await storage.repositories.requests.setStatus(id, status);
    } catch {
      if (mounted.current) { setUpdateError(true); }
    } finally {
      if (mounted.current) { setUpdating(null); }
    }
  }

  return (
    <Screen>
      <FlatList
        data={items}
        keyExtractor={item => item.id}
        refreshing={loading && storage.status === 'ready'}
        onRefresh={() => { refresh(); }}
        contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl}}
        ListHeaderComponent={<>
          <AppText variant="title" style={{marginTop: theme.spacing.xl}}>Message requests</AppText>
          <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>Requests saved on this device. Accepting a request does not verify its sender.</AppText>
          <StorageStatus />
          {error && storage.status === 'ready' ? <View style={{marginTop: theme.spacing.lg}}><AppText variant="body" color="danger">Could not load requests.</AppText><Button title="Try again" variant="secondary" onPress={() => { refresh(); }} style={{marginTop: theme.spacing.sm}} /></View> : null}
          {updateError ? <AppText variant="body" color="danger" style={{marginTop: theme.spacing.md}} accessibilityLiveRegion="polite">Could not update the saved request.</AppText> : null}
        </>}
        renderItem={({item}) => <Surface bordered padded style={{marginTop: theme.spacing.lg}}>
          <AppText variant="heading">{item.displayName}</AppText>
          <AppText variant="caption" color="textSecondary" selectable style={{marginTop: theme.spacing.xs}}>{item.fromVoidId}</AppText>
          <AppText variant="body" style={{marginTop: theme.spacing.md}}>{item.preview}</AppText>
          <AppText variant="caption" color="textMuted" style={{marginTop: theme.spacing.sm}}>{new Date(item.createdAt).toLocaleString()} · {item.status}</AppText>
          {item.status === 'pending' ? <View style={{marginTop: theme.spacing.md}}>
            <Button title="Accept locally" loading={updating === item.id} disabled={updating !== null} onPress={() => { update(item.id, 'accepted'); }} />
            <View style={[styles.actions, {marginTop: theme.spacing.sm}]}>
              <Button title="Decline" variant="secondary" disabled={updating !== null} onPress={() => { update(item.id, 'declined'); }} />
              <Button title="Block locally" variant="danger" disabled={updating !== null} onPress={() => { update(item.id, 'blocked'); }} style={{marginLeft: theme.spacing.sm}} />
            </View>
          </View> : null}
        </Surface>}
        ListEmptyComponent={storage.status === 'ready' && !error ? <View style={{marginTop: theme.spacing.xxxl}}>
          <AppText variant="heading">{loading ? 'Loading requests…' : 'No message requests'}</AppText>
          <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>No requests are saved on this device.</AppText>
        </View> : null}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({actions: {flexDirection: 'row'}});
