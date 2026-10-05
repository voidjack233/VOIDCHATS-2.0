import React from 'react';
import {StyleSheet, View} from 'react-native';
import {useTheme} from '../theme';
import {AppText, Button} from '../ui';
import {StartupLayout} from './components/StartupLayout';

export interface WelcomeScreenProps {
  onContinue: () => void;
}

export function WelcomeScreen({onContinue}: WelcomeScreenProps) {
  const theme = useTheme();

  return (
    <StartupLayout
      footer={<Button title="Get started" onPress={onContinue} fullWidth />}>
      <View style={{marginTop: theme.spacing.xxxl}}>
        <AppText variant="caption" color="accent" style={styles.brand}>
          VOID
        </AppText>
        <AppText variant="display" style={{marginTop: theme.spacing.xxl}}>
          Welcome to VOID
        </AppText>
        <AppText
          variant="body"
          color="textSecondary"
          style={{marginTop: theme.spacing.lg}}>
          Private conversations built around your VOID identity.
        </AppText>
      </View>
    </StartupLayout>
  );
}

const styles = StyleSheet.create({
  brand: {letterSpacing: 2, fontWeight: '700'},
});
