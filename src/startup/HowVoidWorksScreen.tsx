import React from 'react';
import {View} from 'react-native';
import {useTheme} from '../theme';
import {AppText, Button, Divider} from '../ui';
import {StartupLayout} from './components/StartupLayout';

export interface HowVoidWorksScreenProps {
  onContinue: () => void;
  onBack: () => void;
}

export function HowVoidWorksScreen({onContinue, onBack}: HowVoidWorksScreenProps) {
  const theme = useTheme();

  return (
    <StartupLayout
      onBack={onBack}
      footer={<Button title="Continue" onPress={onContinue} fullWidth />}>
      <AppText variant="caption" color="accent" style={{marginTop: theme.spacing.xl}}>
        HOW VOID WORKS
      </AppText>
      <AppText variant="display" style={{marginTop: theme.spacing.md}}>
        Your identity on VOID
      </AppText>
      <AppText
        variant="body"
        color="textSecondary"
        style={{marginTop: theme.spacing.md}}>
        Your persistent account identity has three distinct parts for how you appear and connect on VOID.
      </AppText>

      <View style={{marginTop: theme.spacing.xxxl}}>
        <AppText variant="heading">VOID-ID</AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.sm}}>
          Your stable public identifier lets people find you. It is not a password or a cryptographic key.
        </AppText>
        <Divider style={{marginVertical: theme.spacing.xl}} />

        <AppText variant="heading">Display name</AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.sm}}>
          Your display name is human-readable and cosmetic. You can change it without changing your VOID-ID.
        </AppText>
        <Divider style={{marginVertical: theme.spacing.xl}} />

        <AppText variant="heading">Cryptographic identity</AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.sm}}>
          A separate cryptographic identity will support secure conversations. It is distinct from your public VOID-ID.
        </AppText>
      </View>
    </StartupLayout>
  );
}
