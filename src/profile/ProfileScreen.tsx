import React from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { useAccount } from '../account';
import { useStorage } from '../storage';
import { useTheme } from '../theme';
import { AppText, Divider, Screen } from '../ui';
import { StorageStatus } from '../ui/StorageStatus';

export interface ProfileScreenProps {
  onOpenSettings: () => void;
}

export function ProfileScreen({ onOpenSettings }: ProfileScreenProps) {
  const { colors, spacing } = useTheme();
  const account = useAccount();
  const storage = useStorage();
  const name = account.username?.trim();
  const voidId = storage.status === 'ready' ? storage.identity?.accountVoidId : null;

  return (
    <Screen>
      <AppText
        accessibilityRole="header"
        variant="title"
        style={{ paddingHorizontal: spacing.xl, paddingTop: spacing.xl, paddingBottom: spacing.lg }}>
        Profile
      </AppText>
      <ScrollView contentContainerStyle={{ paddingHorizontal: spacing.xl, paddingBottom: spacing.xxl }}>
        {storage.status === 'ready' ? (
          <>
            {name ? (
              <AppText variant="heading" style={{ marginBottom: spacing.md }}>{name}</AppText>
            ) : null}
            <AppText accessibilityRole="header" variant="label" color="textSecondary">VOID-ID</AppText>
            {voidId ? (
              <AppText variant="title" selectable style={[styles.id, { marginTop: spacing.xs }]}>{voidId}</AppText>
            ) : (
              <AppText color="textSecondary" style={{ marginTop: spacing.xs }}>Not set up yet</AppText>
            )}
          </>
        ) : <StorageStatus />}

        <View style={{ marginTop: spacing.xxl }}>
          <Divider />
          <Pressable
            accessibilityRole="button"
            accessibilityLabel="Open settings"
            onPress={onOpenSettings}
            style={({ pressed }) => [styles.row, {
              paddingVertical: spacing.md,
              backgroundColor: pressed ? colors.surface : 'transparent',
            }]}>
            <AppText style={styles.flex}>Settings</AppText>
            <AppText variant="heading" color="textMuted" accessibilityElementsHidden importantForAccessibility="no">
              ›
            </AppText>
          </Pressable>
          <Divider />
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  id: { fontVariant: ['tabular-nums'] },
  row: { flexDirection: 'row', alignItems: 'center', minHeight: 48 },
  flex: { flex: 1 },
});
