import React from 'react';
import {Pressable, StyleSheet, View} from 'react-native';
import {useTheme} from '../../theme';
import {AppText} from '../../ui';

export type MainTab = 'conversations' | 'requests' | 'contacts';

const tabs: ReadonlyArray<{key: MainTab; label: string}> = [
  {key: 'conversations', label: 'Chats'},
  {key: 'requests', label: 'Requests'},
  {key: 'contacts', label: 'Contacts'},
];

type BottomTabBarProps = {
  selected: MainTab;
  onSelect: (tab: MainTab) => void;
  chatCount: number;
  requestCount: number;
};

function TabIcon({tab, color}: {tab: MainTab; color: string}): React.JSX.Element {
  if (tab === 'conversations') {
    return (
      <View style={styles.iconFrame}>
        <View style={[styles.chatOutline, {borderColor: color}]} />
        <View style={[styles.chatTail, {borderColor: color}]} />
      </View>
    );
  }

  if (tab === 'requests') {
    return (
      <View style={styles.iconFrame}>
        <View style={[styles.requestOutline, {borderColor: color}]} />
        <View style={[styles.requestLine, {backgroundColor: color}]} />
        <View style={[styles.requestDot, {backgroundColor: color}]} />
      </View>
    );
  }

  return (
    <View style={styles.iconFrame}>
      <View style={[styles.contactHead, {borderColor: color}]} />
      <View style={[styles.contactShoulders, {borderColor: color}]} />
      <View style={[styles.contactSecondHead, {borderColor: color}]} />
    </View>
  );
}

export function BottomTabBar({
  selected,
  onSelect,
  chatCount,
  requestCount,
}: BottomTabBarProps): React.JSX.Element {
  const {colors, radii, spacing} = useTheme();

  return (
    <View
      accessibilityRole="tablist"
      style={[
        styles.container,
        {
          backgroundColor: colors.background,
          borderTopColor: colors.border,
          paddingHorizontal: spacing.md,
          paddingVertical: spacing.sm,
          gap: spacing.sm,
        },
      ]}>
      {tabs.map(tab => {
        const isSelected = selected === tab.key;
        const count = tab.key === 'conversations' ? chatCount : tab.key === 'requests' ? requestCount : 0;
        const color = isSelected ? colors.accent : colors.textSecondary;
        const countLabel = tab.key === 'conversations' ? 'unread message' : 'request';
        const accessibilityLabel = count > 0
          ? `${tab.label}, ${count} ${countLabel}${count === 1 ? '' : 's'}`
          : tab.label;

        return (
          <Pressable
            key={tab.key}
            accessibilityRole="tab"
            accessibilityLabel={accessibilityLabel}
            accessibilityState={{selected: isSelected}}
            onPress={() => onSelect(tab.key)}
            style={({pressed}) => [styles.tab, {
              backgroundColor: pressed ? colors.surfaceElevated : isSelected ? colors.surface : 'transparent',
              borderRadius: radii.lg,
              gap: spacing.xs,
              padding: spacing.xs,
            }]}>
            <View
              accessibilityElementsHidden
              importantForAccessibility="no-hide-descendants"
              style={styles.iconPill}>
              <TabIcon tab={tab.key} color={color} />
            </View>
            <View style={[styles.labelRow, {gap: spacing.xs}]}>
              <AppText
                variant="caption"
                color={isSelected ? 'accent' : 'textSecondary'}
                style={[styles.label, isSelected && styles.selectedLabel]}>
                {tab.label}
              </AppText>
              {count > 0 ? (
                <View
                  accessibilityElementsHidden
                  importantForAccessibility="no-hide-descendants"
                  style={[styles.badge, {
                    backgroundColor: colors.accent,
                    borderRadius: radii.full,
                    paddingHorizontal: spacing.xs,
                  }]}>
                  <AppText variant="caption" color="accentText" style={styles.badgeText}>
                    {count > 99 ? '99+' : count}
                  </AppText>
                </View>
              ) : null}
            </View>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  badge: {minWidth: 20, minHeight: 20, maxWidth: '100%', alignItems: 'center', justifyContent: 'center'},
  badgeText: {fontWeight: '700', fontVariant: ['tabular-nums'], textAlign: 'center'},
  container: {
    flexDirection: 'row',
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  tab: {
    flex: 1,
    minWidth: 0,
    minHeight: 60,
    alignItems: 'center',
    justifyContent: 'center',
  },
  iconPill: {width: 32, height: 26, alignItems: 'center', justifyContent: 'center'},
  labelRow: {alignItems: 'center', flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'center', width: '100%'},
  label: {
    flexShrink: 1,
    textAlign: 'center',
  },
  selectedLabel: {fontWeight: '600'},
  iconFrame: {
    width: 24,
    height: 24,
    alignItems: 'center',
    justifyContent: 'center',
  },
  chatOutline: {
    width: 20,
    height: 16,
    borderWidth: 2,
    borderRadius: 5,
    marginBottom: 3,
  },
  chatTail: {
    position: 'absolute',
    width: 5,
    height: 5,
    left: 4,
    bottom: 1,
    borderLeftWidth: 2,
    borderBottomWidth: 2,
    transform: [{skewY: '-30deg'}],
  },
  requestOutline: {
    width: 20,
    height: 17,
    borderWidth: 2,
    borderRadius: 4,
    marginTop: 2,
  },
  requestLine: {
    position: 'absolute',
    top: 8,
    left: 6,
    width: 8,
    height: 2,
    borderRadius: 1,
  },
  requestDot: {
    position: 'absolute',
    top: 8,
    right: 5,
    width: 3,
    height: 3,
    borderRadius: 2,
  },
  contactHead: {
    position: 'absolute',
    width: 8,
    height: 8,
    left: 4,
    top: 3,
    borderWidth: 2,
    borderRadius: 5,
  },
  contactShoulders: {
    position: 'absolute',
    width: 14,
    height: 8,
    left: 1,
    bottom: 2,
    borderWidth: 2,
    borderBottomWidth: 0,
    borderTopLeftRadius: 8,
    borderTopRightRadius: 8,
  },
  contactSecondHead: {
    position: 'absolute',
    width: 7,
    height: 7,
    right: 3,
    top: 5,
    borderWidth: 2,
    borderRadius: 5,
  },
});
