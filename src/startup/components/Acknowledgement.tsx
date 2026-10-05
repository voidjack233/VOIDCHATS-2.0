import React from 'react';
import {Pressable, StyleSheet, View} from 'react-native';
import {useTheme} from '../../theme';
import {AppText} from '../../ui';

interface AcknowledgementProps {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}

export function Acknowledgement({
  label,
  checked,
  onChange,
}: AcknowledgementProps) {
  const theme = useTheme();

  return (
    <Pressable
      accessibilityRole="checkbox"
      accessibilityLabel={label}
      accessibilityState={{checked}}
      onPress={() => onChange(!checked)}
      style={({pressed}) => [
        styles.row,
        {
          paddingVertical: theme.spacing.md,
          gap: theme.spacing.md,
          opacity: pressed ? 0.7 : 1,
        },
      ]}>
      <View
        style={[
          styles.checkbox,
          {
            borderColor: checked ? theme.colors.accent : theme.colors.border,
            borderRadius: theme.radii.sm,
          },
          checked && {backgroundColor: theme.colors.accent},
        ]}>
        {checked ? (
          <AppText variant="label" color="accentText">
            ✓
          </AppText>
        ) : null}
      </View>
      <AppText color="textSecondary" style={styles.label}>
        {label}
      </AppText>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: {flexDirection: 'row', alignItems: 'flex-start', minHeight: 44},
  checkbox: {
    width: 22,
    height: 22,
    marginTop: 2,
    backgroundColor: 'transparent',
    borderWidth: 1.5,
    alignItems: 'center',
    justifyContent: 'center',
  },
  label: {flex: 1},
});
