import React from 'react';
import {StyleSheet, View} from 'react-native';
import type {ViewProps} from 'react-native';
import {useTheme} from '../theme';

export interface ScreenProps extends ViewProps {
  padded?: boolean;
}

export function Screen({padded = false, style, ...rest}: ScreenProps) {
  const {colors, spacing} = useTheme();

  return (
    <View
      {...rest}
      style={[
        styles.container,
        {backgroundColor: colors.background},
        padded && {paddingHorizontal: spacing.lg},
        style,
      ]}
    />
  );
}

const styles = StyleSheet.create({
  container: {flex: 1},
});
