import React from 'react';
import {StyleSheet, View} from 'react-native';
import type {ViewProps} from 'react-native';
import {useTheme} from '../theme';

export interface SurfaceProps extends ViewProps {
  elevated?: boolean;
  bordered?: boolean;
  padded?: boolean;
}

export function Surface({
  elevated = false,
  bordered = false,
  padded = false,
  style,
  ...rest
}: SurfaceProps) {
  const {colors, radii, spacing} = useTheme();

  return (
    <View
      {...rest}
      style={[
        {
          backgroundColor: elevated ? colors.surfaceElevated : colors.surface,
          borderColor: colors.border,
          borderRadius: radii.md,
        },
        bordered && styles.bordered,
        padded && {padding: spacing.lg},
        style,
      ]}
    />
  );
}

const styles = StyleSheet.create({
  bordered: {borderWidth: 1},
});
