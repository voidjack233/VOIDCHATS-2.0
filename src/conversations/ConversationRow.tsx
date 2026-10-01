import React from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Avatar } from '../ui';
import type { Conversation } from './types';

interface ConversationRowProps {
  conversation: Conversation;
  onPress: () => void;
}

export function ConversationRow({ conversation, onPress }: ConversationRowProps) {
  const theme = useTheme();

  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Open ${conversation.name}`}
      onPress={onPress}
      style={({ pressed }) => [styles.row, {
        paddingVertical: theme.spacing.lg,
        opacity: pressed ? 0.7 : 1,
      }]}
    >
      <Avatar name={conversation.name} size={48} />
      <View style={[styles.content, { marginLeft: theme.spacing.md }]}>
        <View style={styles.headingRow}>
          <AppText variant="body" numberOfLines={1} style={styles.headingText}>
            {conversation.name}
          </AppText>
          <AppText variant="caption" color="textMuted" style={{ marginLeft: theme.spacing.sm }}>
            {conversation.updatedAtLabel}
          </AppText>
        </View>
        <View style={[styles.previewRow, { marginTop: theme.spacing.xs }]}>
          <AppText variant="body" color="textSecondary" numberOfLines={1} style={styles.previewText}>
            {conversation.preview}
          </AppText>
          {conversation.unreadCount > 0 ? (
            <View
              style={[styles.badge, {
                borderRadius: theme.radii.full,
                backgroundColor: theme.colors.accent,
                paddingHorizontal: theme.spacing.xs,
                marginLeft: theme.spacing.sm,
              }]}
            >
              <AppText variant="caption" color="accentText" style={styles.badgeText}>
                {conversation.unreadCount}
              </AppText>
            </View>
          ) : null}
        </View>
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: { flexDirection: 'row', alignItems: 'center' },
  content: { flex: 1 },
  headingRow: { flexDirection: 'row', alignItems: 'baseline' },
  headingText: { flex: 1, fontWeight: '600' },
  previewRow: { flexDirection: 'row', alignItems: 'center' },
  previewText: { flex: 1 },
  badge: { minWidth: 20, height: 20, alignItems: 'center', justifyContent: 'center' },
  badgeText: { fontWeight: '700' },
});
