import React from 'react';
import {View} from 'react-native';
import {useTheme} from '../theme';
import {AppText, Button} from '../ui';
import {Acknowledgement} from './components/Acknowledgement';
import {StartupLayout} from './components/StartupLayout';

export interface SecurityNoticeScreenProps {
  acknowledged: boolean;
  onAcknowledgedChange: (value: boolean) => void;
  onContinue: () => void;
  onBack: () => void;
}

export function SecurityNoticeScreen({
  acknowledged,
  onAcknowledgedChange,
  onContinue,
  onBack,
}: SecurityNoticeScreenProps) {
  const theme = useTheme();

  const handleContinue = () => {
    if (acknowledged) {
      onContinue();
    }
  };

  return (
    <StartupLayout
      onBack={onBack}
      footer={
        <Button
          title="Continue"
          onPress={handleContinue}
          disabled={!acknowledged}
          fullWidth
        />
      }>
      <AppText variant="caption" color="warning" style={{marginTop: theme.spacing.xl}}>
        SECURITY
      </AppText>
      <AppText variant="display" style={{marginTop: theme.spacing.md}}>
        Protect your VOID identity
      </AppText>
      <View style={{marginTop: theme.spacing.xl}}>
        <AppText variant="body" color="textSecondary">
          Your cryptographic identity helps establish and verify secure conversations. Its private material stays on your device.
        </AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.lg}}>
          Some identity or recovery information may be needed when you move devices or recover access. Keep any information you receive safe and private.
        </AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.lg}}>
          VOID cannot recreate private cryptographic material that exists only on your device.
        </AppText>
      </View>
      <View style={{marginTop: theme.spacing.xxxl}}>
        <Acknowledgement
          label="I understand that my VOID identity and recovery information are important."
          checked={acknowledged}
          onChange={onAcknowledgedChange}
        />
      </View>
    </StartupLayout>
  );
}
