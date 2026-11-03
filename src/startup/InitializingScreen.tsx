import React, {useEffect, useRef} from 'react';
import {ActivityIndicator, StyleSheet, View} from 'react-native';
import {useAccount} from '../account';
import {useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button} from '../ui';
import {StartupLayout} from './components/StartupLayout';

export interface InitializingScreenProps {
  onBack: () => void;
  onPreview: () => void;
  onBusyChange: (busy: boolean) => void;
}

export function InitializingScreen({onBack, onPreview, onBusyChange}: InitializingScreenProps) {
  const theme = useTheme();
  const account = useAccount();
  const storage = useStorage();
  const submitting = useRef(false);
  const busy = !['idle', 'error', 'ready'].includes(account.status);
  const nativeBuildRequired = storage.status === 'error' &&
    ['UNSUPPORTED_PLATFORM', 'NATIVE_MODULE_UNAVAILABLE', 'NATIVE_MODULE_INCOMPATIBLE'].includes(storage.errorCode ?? '');
  const progress = account.status === 'preparing' ? 'Protecting your identity on this device…'
    : account.status === 'challenge' ? 'Connecting to VOID…'
    : account.status === 'signing' ? 'Confirming your device identity…'
    : account.status === 'registering' ? 'Creating your VOID account…'
    : account.status === 'saving' ? 'Saving your account on this device…'
    : account.status === 'ready' ? 'Your account is ready.' : null;

  useEffect(() => {
    onBusyChange(busy);
    return () => onBusyChange(false);
  }, [busy, onBusyChange]);

  const initialize = async () => {
    if (submitting.current || busy || nativeBuildRequired || account.status === 'ready') { return; }
    submitting.current = true;
    try { await account.initialize(); }
    catch { /* The account provider supplies a safe error message below. */ }
    finally { submitting.current = false; }
  };

  return (
    <StartupLayout
      onBack={busy ? undefined : onBack}
      footer={
        __DEV__ ? (
          <View>
            <AppText
              variant="caption"
              color="textMuted"
              style={{marginBottom: theme.spacing.md}}>
              Development preview only. Your account remains uninitialized.
            </AppText>
            <Button
              title="Preview VOID"
              variant="ghost"
              size="sm"
              hitSlop={3}
              disabled={busy || !account.canPreview}
              onPress={onPreview}
            />
          </View>
        ) : null
      }>
      <AppText variant="caption" color="accent" style={{marginTop: theme.spacing.xl}}>
        INITIALIZATION
      </AppText>
      <AppText variant="title" style={{marginTop: theme.spacing.md}}>
        {nativeBuildRequired ? 'Open the VOID Android app' : 'Set up your account'}
      </AppText>
      <AppText
        variant="body"
        color="textSecondary"
        style={{marginTop: theme.spacing.lg}}>
        {nativeBuildRequired
          ? 'Account creation needs VOID’s encrypted storage and identity protection. These are included in the custom Android app. Expo Go can preview the screens, but cannot create an account.'
          : 'Register this device’s protected identity to get your VOID-ID. Your private identity keys stay on this device.'}
      </AppText>
      {nativeBuildRequired ? (
        <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.lg}}>
          Install and open the latest VOID Android build, then return to account setup.
          {__DEV__ ? ' You can keep exploring the screens with Preview VOID below.' : ''}
        </AppText>
      ) : null}
      {progress ? (
        <View style={[styles.progress, {marginTop: theme.spacing.xl}]}>
          {busy ? <ActivityIndicator color={theme.colors.accent} style={{marginRight: theme.spacing.md}} /> : null}
          <AppText color="textSecondary" accessibilityLiveRegion="polite" style={styles.progressText}>{progress}</AppText>
        </View>
      ) : null}
      {account.error ? (
        <AppText color="danger" accessibilityRole="alert" accessibilityLiveRegion="polite" style={{marginTop: theme.spacing.xl}}>
          {account.error}
        </AppText>
      ) : null}
      {account.connectionDetails ? (
        <AppText variant="caption" color="textMuted" selectable style={{marginTop: theme.spacing.sm}}>
          {account.connectionDetails}
        </AppText>
      ) : null}
      {!busy && !nativeBuildRequired && (storage.status === 'locked' || storage.status === 'error') ? (
        <View style={{marginTop: theme.spacing.lg}}>
          <AppText variant="caption" color="textSecondary">{storage.message ?? 'Reopen your encrypted local storage to continue.'}</AppText>
          <Button title="Retry opening local storage" variant="secondary" onPress={() => { storage.openExisting().catch(() => {}); }} style={{marginTop: theme.spacing.md}} />
        </View>
      ) : null}
      <Button
        title={nativeBuildRequired ? 'Android build required' : account.status === 'error' ? 'Retry account setup' : 'Create account'}
        fullWidth
        loading={busy}
        disabled={nativeBuildRequired || storage.status === 'opening' || account.status === 'ready'}
        onPress={() => { initialize().catch(() => {}); }}
        style={{marginTop: theme.spacing.xl}}
      />
    </StartupLayout>
  );
}

const styles = StyleSheet.create({
  progress: {flexDirection: 'row', alignItems: 'center'},
  progressText: {flex: 1},
});
