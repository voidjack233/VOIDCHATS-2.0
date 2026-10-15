import React from 'react';
import {View} from 'react-native';
import {useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button} from './index';

export function StorageStatus() {
  const storage = useStorage();
  const theme = useTheme();
  if (storage.status === 'ready') { return null; }
  const title = storage.status === 'opening' ? 'Opening local storage'
    : storage.status === 'not_initialized' ? 'Local storage not set up'
    : storage.status === 'locked' ? 'Local storage closed' : 'Local storage unavailable';
  return (
    <View style={{paddingVertical: theme.spacing.xl}}>
      <AppText variant="heading">{title}</AppText>
      <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>
        {storage.message ?? (storage.status === 'not_initialized'
          ? 'Set up local storage in Settings to keep this device’s local data.'
          : 'Your local data is not available while storage is closed.')}
      </AppText>
      {storage.status === 'locked' || storage.status === 'error' ? (
        <Button title="Open existing storage" variant="secondary" onPress={() => { storage.openExisting().catch(() => {}); }} style={{marginTop: theme.spacing.lg}} />
      ) : null}
    </View>
  );
}
