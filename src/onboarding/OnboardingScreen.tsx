import React from 'react';
import { StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Button, Screen } from '../ui';

export interface OnboardingScreenProps {
  onContinue: () => void;
}

export function OnboardingScreen({ onContinue }: OnboardingScreenProps) {
  const theme = useTheme();

  return (
    <Screen style={[styles.screen, { paddingHorizontal: theme.spacing.xl }]}>
      <View style={{ paddingTop: theme.spacing.xxxl }}>
        <View
          style={[styles.mark, {
            borderRadius: theme.radii.lg,
            borderColor: theme.colors.accent,
          }]}
        >
          <AppText variant="title" color="accent">V</AppText>
        </View>
        <AppText variant="caption" color="accent" style={[styles.brand, { marginTop: theme.spacing.lg }]}>
          VOID
        </AppText>
      </View>
      <View>
        <AppText variant="display">Conversation starts here.</AppText>
        <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.lg }}>
          A calm place for direct and group conversations, built around your VOID-ID.
        </AppText>
      </View>
      <View style={{ paddingBottom: theme.spacing.xxxl }}>
        <Button title="Explore VOID" onPress={onContinue} fullWidth />
        <AppText variant="caption" color="textMuted" style={[styles.centerText, { marginTop: theme.spacing.md }]}>
          VOID 2.0 preview
        </AppText>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { justifyContent: 'space-between' },
  mark: {
    width: 56,
    height: 56,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  brand: { letterSpacing: 2, fontWeight: '700' },
  centerText: { textAlign: 'center' },
});
