import React from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Divider, Screen } from '../ui';

export interface ConversationsScreenProps {
  onOpenRequests: () => void;
  onOpenContacts: () => void;
}

export function ConversationsScreen({
  onOpenRequests,
  onOpenContacts,
}: ConversationsScreenProps) {
  const theme = useTheme();

  return (
    <Screen>
      <View style={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl }}>
        <View style={styles.headerRow}>
          <View>
            <AppText variant="caption" color="accent" style={styles.brand}>
              VOID
            </AppText>
            <AppText variant="title" style={{ marginTop: theme.spacing.xs }}>
              Conversations
            </AppText>
          </View>
          <Pressable
            accessibilityRole="button"
            onPress={onOpenContacts}
            style={({ pressed }) => ({ padding: theme.spacing.sm, opacity: pressed ? 0.65 : 1 })}
          >
            <AppText variant="label" color="accent">Find people</AppText>
          </Pressable>
        </View>
        <Pressable
          accessibilityRole="button"
          onPress={onOpenRequests}
          style={({ pressed }) => [styles.shortcut, {
            marginTop: theme.spacing.xl,
            paddingVertical: theme.spacing.lg,
            opacity: pressed ? 0.65 : 1,
          }]}
        >
          <AppText variant="label" style={styles.flex}>Message requests</AppText>
          <AppText variant="body" color="textMuted">›</AppText>
        </Pressable>
      </View>
      <Divider />
      <ScrollView contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}>
        <View style={[styles.emptyState, { paddingTop: theme.spacing.xxxl }]}>
          <AppText variant="heading">No conversations yet</AppText>
          <AppText variant="body" color="textSecondary" style={[styles.emptyDescription, { marginTop: theme.spacing.sm }]}>
            Your conversations will appear here once messaging is connected.
          </AppText>
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  headerRow: { flexDirection: 'row', alignItems: 'flex-end', justifyContent: 'space-between' },
  brand: { letterSpacing: 2, fontWeight: '700' },
  shortcut: { flexDirection: 'row', alignItems: 'center' },
  flex: { flex: 1 },
  emptyState: { alignItems: 'center' },
  emptyDescription: { textAlign: 'center' },
});
