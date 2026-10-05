import React from 'react';
import {View} from 'react-native';
import {useTheme} from '../theme';
import {AppText, Button} from '../ui';
import {Acknowledgement} from './components/Acknowledgement';
import {StartupLayout} from './components/StartupLayout';

export interface TermsScreenProps {
  accepted: boolean;
  onAcceptedChange: (value: boolean) => void;
  onInitialize: () => void;
  onBack: () => void;
}

export function TermsScreen({
  accepted,
  onAcceptedChange,
  onInitialize,
  onBack,
}: TermsScreenProps) {
  const theme = useTheme();

  function handleInitialize() {
    if (accepted) {
      onInitialize();
    }
  }

  return (
    <StartupLayout
      onBack={onBack}
      footer={
        <Button
          title="Initialize VOID"
          fullWidth
          disabled={!accepted}
          onPress={handleInitialize}
        />
      }>
      <AppText variant="caption" color="accent" style={{marginTop: theme.spacing.xl}}>
        TERMS
      </AppText>
      <AppText variant="title" style={{marginTop: theme.spacing.md}}>
        Terms of Service
      </AppText>
      <View style={{marginTop: theme.spacing.xl}}>
        <AppText variant="label" color="accent">Development copy</AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.sm}}>
          The final Terms of Service are not available in this preview. This
          acknowledgement is for UI development only; no account is created.
        </AppText>
      </View>
      <View style={{marginTop: theme.spacing.xxxl}}>
        <Acknowledgement
          label="I agree to the Terms of Service"
          checked={accepted}
          onChange={onAcceptedChange}
        />
      </View>
    </StartupLayout>
  );
}
