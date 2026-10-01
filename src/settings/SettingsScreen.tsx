import React from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Divider, IconButton, Screen } from '../ui';

export interface SettingsScreenProps {
  onBack: () => void;
}

interface SettingRowProps {
  label: string;
  value: string;
}

function SettingRow({ label, value }: SettingRowProps) {
  const theme = useTheme();
  return (
    <View style={[styles.row, { paddingVertical: theme.spacing.lg }]}>
      <AppText variant="body" style={styles.flex}>{label}</AppText>
      <AppText variant="body" color="textSecondary" style={{ marginLeft: theme.spacing.lg }}>
        {value}
      </AppText>
    </View>
  );
}

export function SettingsScreen({ onBack }: SettingsScreenProps) {
  const theme = useTheme();

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}>
        <View style={[styles.row, { marginTop: theme.spacing.md, marginBottom: theme.spacing.lg }]}>
          <IconButton
            accessibilityLabel="Back to profile"
            onPress={onBack}
            icon={<AppText variant="heading">‹</AppText>}
            style={{ marginLeft: -theme.spacing.md, marginRight: theme.spacing.sm }}
          />
          <AppText variant="title">Settings</AppText>
        </View>

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginBottom: theme.spacing.sm }]}>
          APPEARANCE
        </AppText>
        <Divider />
        <SettingRow label="Theme" value="Dark" />
        <Divider />

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginTop: theme.spacing.xxl, marginBottom: theme.spacing.sm }]}>
          ACCOUNT
        </AppText>
        <Divider />
        <SettingRow label="VOID-ID" value="Not set up" />
        <Divider />

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginTop: theme.spacing.xxl, marginBottom: theme.spacing.sm }]}>
          ABOUT
        </AppText>
        <Divider />
        <SettingRow label="VOID" value="2.0 preview" />
        <Divider />
        <AppText variant="caption" color="textMuted" style={{ marginTop: theme.spacing.xl }}>
          More preferences are coming soon.
        </AppText>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  row: { flexDirection: 'row', alignItems: 'center' },
  flex: { flex: 1 },
  sectionLabel: { letterSpacing: 1 },
});
