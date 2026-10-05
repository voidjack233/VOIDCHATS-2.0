import React from 'react';
import {View} from 'react-native';
import {useTheme} from '../theme';
import {AppText, Button} from '../ui';
import {StartupLayout} from './components/StartupLayout';

export interface InitializingScreenProps {
  onBack: () => void;
  onPreview: () => void;
}

export function InitializingScreen({onBack, onPreview}: InitializingScreenProps) {
  const theme = useTheme();

  return (
    <StartupLayout
      onBack={onBack}
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
              onPress={onPreview}
            />
          </View>
        ) : null
      }>
      <AppText variant="caption" color="accent" style={{marginTop: theme.spacing.xl}}>
        INITIALIZATION
      </AppText>
      <AppText variant="title" style={{marginTop: theme.spacing.md}}>
        Setting up VOID
      </AppText>
      <AppText
        variant="body"
        color="textSecondary"
        style={{marginTop: theme.spacing.lg}}>
        Account initialization is not connected yet.
      </AppText>
    </StartupLayout>
  );
}
