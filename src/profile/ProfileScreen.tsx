import React from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Avatar, Divider, Screen, Surface } from '../ui';

export interface ProfileScreenProps {
  onOpenSettings: () => void;
}

export function ProfileScreen({ onOpenSettings }: ProfileScreenProps) {
  const theme = useTheme();

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}>
        <AppText variant="title">Profile</AppText>
        <View style={[styles.profileHeader, { paddingVertical: theme.spacing.xxxl }]}>
          <Avatar name="VOID" size={76} />
          <AppText variant="heading" style={{ marginTop: theme.spacing.lg }}>
            Your space on VOID
          </AppText>
          <AppText variant="body" color="textSecondary" style={[styles.centerText, { marginTop: theme.spacing.sm }]}>
            Profile setup is coming soon.
          </AppText>
        </View>

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginBottom: theme.spacing.sm }]}>
          IDENTITY
        </AppText>
        <Surface bordered padded>
          <AppText variant="label">VOID-ID</AppText>
          <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.xs }}>
            Not set up yet
          </AppText>
        </Surface>

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginTop: theme.spacing.xxl, marginBottom: theme.spacing.sm }]}>
          PREFERENCES
        </AppText>
        <Divider />
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Open settings"
          onPress={onOpenSettings}
          style={({ pressed }) => [styles.settingsRow, {
            paddingVertical: theme.spacing.lg,
            opacity: pressed ? 0.7 : 1,
          }]}
        >
          <AppText variant="body" style={styles.flex}>Settings</AppText>
          <AppText variant="body" color="textMuted">›</AppText>
        </Pressable>
        <Divider />
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  profileHeader: { alignItems: 'center' },
  centerText: { textAlign: 'center' },
  sectionLabel: { letterSpacing: 1 },
  settingsRow: { flexDirection: 'row', alignItems: 'center' },
  flex: { flex: 1 },
});
