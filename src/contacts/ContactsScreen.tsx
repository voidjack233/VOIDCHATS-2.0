import React from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Button, Screen, Surface } from '../ui';

export function ContactsScreen() {
  const theme = useTheme();

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}>
        <AppText variant="title">Contacts</AppText>
        <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
          People you know, found by VOID-ID.
        </AppText>

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginTop: theme.spacing.xxl, marginBottom: theme.spacing.sm }]}>
          SAVED CONTACTS
        </AppText>
        <View style={[styles.emptyState, { paddingVertical: theme.spacing.xl }]}>
          <AppText variant="heading">No contacts yet</AppText>
          <AppText variant="body" color="textSecondary" style={[styles.emptyDescription, { marginTop: theme.spacing.sm }]}>
            Saved contacts will appear here once account services are connected.
          </AppText>
        </View>

        <Surface bordered padded style={{ marginTop: theme.spacing.xxl }}>
          <AppText variant="heading">Find someone by VOID-ID</AppText>
          <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
            VOID-ID lookup is not connected yet.
          </AppText>
          <Button
            title="Search VOID-ID"
            variant="secondary"
            size="sm"
            disabled
            style={{ marginTop: theme.spacing.lg }}
          />
        </Surface>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  sectionLabel: { letterSpacing: 1 },
  emptyState: { alignItems: 'center' },
  emptyDescription: { textAlign: 'center' },
});
