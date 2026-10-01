import React from 'react';
import {StyleSheet, View} from 'react-native';
import type {ViewProps} from 'react-native';
import {useTheme} from '../theme';

export interface DividerProps extends ViewProps {
  inset?: number;
}

export function Divider({inset = 0, style, ...rest}: DividerProps) {
  const {colors} = useTheme();

  return (
    <View
      {...rest}
      accessibilityRole="none"
      style={[
        styles.line,
        {backgroundColor: colors.border, marginHorizontal: inset},
        style,
      ]}
    />
  );
}

const styles = StyleSheet.create({
  line: {height: 1},
});
