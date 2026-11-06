import React from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Avatar } from '../ui';
import type { Conversation } from './types';

interface ConversationRowProps {
  conversation: Conversation;
  onPress: () => void;
}

export function ConversationRow({conversation, onPress}: ConversationRowProps) {
  const {colors, radii, spacing} = useTheme();
  const {name, preview, updatedAtLabel, unreadCount} = conversation;
  const unread = unreadCount > 0;

  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={`Open ${name}`}
      accessibilityValue={unread ? {text: `${unreadCount} unread ${unreadCount === 1 ? 'message' : 'messages'}`} : undefined}
      onPress={onPress}
      style={({pressed}) => [styles.row, {
        paddingVertical: spacing.md,
        paddingHorizontal: spacing.md,
        marginHorizontal: -spacing.md,
        borderRadius: radii.md,
        backgroundColor: pressed ? colors.surface : 'transparent',
      }]}>
      <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        <Avatar name={name} size={44} />
      </View>
      <View style={[styles.content, {marginLeft: spacing.md}]}>
        <View style={styles.copy}>
          <AppText numberOfLines={2} style={unread ? styles.unreadName : styles.name}>
            {name}
          </AppText>
          {preview ? (
            <AppText variant="caption" color={unread ? 'textPrimary' : 'textSecondary'} numberOfLines={2}
              style={{marginTop: spacing.xs}}>
              {preview}
            </AppText>
          ) : null}
        </View>
        <View style={[styles.meta, {marginLeft: spacing.sm}]}>
          <AppText variant="caption" color="textSecondary" style={styles.timestamp}>{updatedAtLabel}</AppText>
          {unread ? (
            <View style={[styles.badge, {
              borderRadius: radii.full,
              backgroundColor: colors.accent,
              paddingHorizontal: spacing.sm,
              paddingVertical: spacing.xs,
              marginTop: spacing.xs,
            }]}>
              <AppText variant="caption" color="accentText" numberOfLines={1} style={styles.badgeText}>
                {unreadCount > 99 ? '99+' : unreadCount}
              </AppText>
            </View>
          ) : null}
        </View>
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: {flexDirection: 'row', alignItems: 'center', minHeight: 64},
  content: {flex: 1, flexDirection: 'row', alignItems: 'flex-start', minWidth: 0},
  copy: {flex: 1, minWidth: 0},
  name: {fontWeight: '600'},
  unreadName: {fontWeight: '700'},
  meta: {alignItems: 'flex-end', maxWidth: '42%'},
  timestamp: {textAlign: 'right'},
  badge: {minWidth: 24, minHeight: 24, alignItems: 'center', justifyContent: 'center'},
  badgeText: {fontWeight: '700', fontVariant: ['tabular-nums']},
});
