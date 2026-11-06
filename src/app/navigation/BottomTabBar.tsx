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
  const theme = useTheme();

  return (
    <View
      accessibilityRole="tablist"
      style={[
        styles.container,
        {
          backgroundColor: theme.colors.background,
          borderTopColor: theme.colors.border,
        },
      ]}>
      {tabs.map(tab => {
        const isSelected = selected === tab.key;
        const count = tab.key === 'conversations' ? chatCount : tab.key === 'requests' ? requestCount : 0;
        const color = isSelected
          ? theme.colors.accent
          : theme.colors.textMuted;

        return (
          <Pressable
            key={tab.key}
            accessibilityRole="tab"
            accessibilityLabel={count ? `${tab.label}, ${count} ${tab.key === 'conversations' ? 'unread messages' : 'requests'}` : tab.label}
            accessibilityState={{selected: isSelected}}
            onPress={() => onSelect(tab.key)}
            style={styles.tab}>
            <View>
              <TabIcon tab={tab.key} color={color} />
              {count > 0 ? <View style={[styles.badge, {backgroundColor: theme.colors.accent}]}><AppText color="accentText" style={styles.badgeText}>{count > 99 ? '99+' : count}</AppText></View> : null}
            </View>
            <AppText
              variant="caption"
              color={isSelected ? 'accent' : 'textMuted'}
              style={styles.label}>
              {tab.label}
            </AppText>
          </Pressable>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  badge: {position: 'absolute', top: -5, left: 16, minWidth: 18, height: 18, borderRadius: 9, paddingHorizontal: 4, alignItems: 'center', justifyContent: 'center'},
  badgeText: {fontSize: 10, lineHeight: 13, fontWeight: '700'},
  container: {
    flexDirection: 'row',
    borderTopWidth: StyleSheet.hairlineWidth,
    paddingTop: 9,
    paddingBottom: 4,
  },
  tab: {
    flex: 1,
    minHeight: 49,
    alignItems: 'center',
    justifyContent: 'center',
    gap: 5,
  },
  label: {
    fontSize: 11,
  },
  iconFrame: {
    width: 24,
    height: 23,
    alignItems: 'center',
    justifyContent: 'center',
  },
  chatOutline: {
    width: 20,
    height: 16,
    borderWidth: 1.7,
    borderRadius: 5,
    marginBottom: 3,
  },
  chatTail: {
    position: 'absolute',
    width: 5,
    height: 5,
    left: 4,
    bottom: 1,
    borderLeftWidth: 1.7,
    borderBottomWidth: 1.7,
    transform: [{skewY: '-30deg'}],
  },
  requestOutline: {
    width: 20,
    height: 17,
    borderWidth: 1.7,
    borderRadius: 4,
    marginTop: 2,
  },
  requestLine: {
    position: 'absolute',
    top: 8,
    left: 6,
    width: 8,
    height: 1.7,
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
    borderWidth: 1.6,
    borderRadius: 5,
  },
  contactShoulders: {
    position: 'absolute',
    width: 14,
    height: 8,
    left: 1,
    bottom: 2,
    borderWidth: 1.6,
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
    borderWidth: 1.6,
    borderRadius: 5,
  },
});
