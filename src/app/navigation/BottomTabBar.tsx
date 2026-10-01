import React from 'react';
import {Pressable, StyleSheet, View} from 'react-native';
import {useTheme} from '../../theme';
import {AppText} from '../../ui';

export type MainTab = 'conversations' | 'requests' | 'contacts' | 'profile';

const tabs: ReadonlyArray<{key: MainTab; label: string}> = [
  {key: 'conversations', label: 'Chats'},
  {key: 'requests', label: 'Requests'},
  {key: 'contacts', label: 'Contacts'},
  {key: 'profile', label: 'Profile'},
];

type BottomTabBarProps = {
  selected: MainTab;
  onSelect: (tab: MainTab) => void;
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

  if (tab === 'contacts') {
    return (
      <View style={styles.iconFrame}>
        <View style={[styles.contactHead, {borderColor: color}]} />
        <View style={[styles.contactShoulders, {borderColor: color}]} />
        <View style={[styles.contactSecondHead, {borderColor: color}]} />
      </View>
    );
  }

  return (
    <View style={styles.iconFrame}>
      <View style={[styles.profileHead, {borderColor: color}]} />
      <View style={[styles.profileShoulders, {borderColor: color}]} />
    </View>
  );
}

export function BottomTabBar({
  selected,
  onSelect,
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
        const color = isSelected
          ? theme.colors.accent
          : theme.colors.textMuted;

        return (
          <Pressable
            key={tab.key}
            accessibilityRole="tab"
            accessibilityLabel={tab.label}
            accessibilityState={{selected: isSelected}}
            onPress={() => onSelect(tab.key)}
            style={styles.tab}>
            <TabIcon tab={tab.key} color={color} />
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
  profileHead: {
    position: 'absolute',
    width: 9,
    height: 9,
    top: 2,
    borderWidth: 1.7,
    borderRadius: 6,
  },
  profileShoulders: {
    position: 'absolute',
    width: 18,
    height: 9,
    bottom: 2,
    borderWidth: 1.7,
    borderBottomWidth: 0,
    borderTopLeftRadius: 10,
    borderTopRightRadius: 10,
  },
});
