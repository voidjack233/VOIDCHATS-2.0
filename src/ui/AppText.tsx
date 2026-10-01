import React from 'react';
import {Text} from 'react-native';
import type {TextProps} from 'react-native';
import {useTheme} from '../theme';
import type {TextVariant, ThemeColor} from '../theme';

export interface AppTextProps extends TextProps {
  variant?: TextVariant;
  color?: ThemeColor;
}

export function AppText({
  variant = 'body',
  color = 'textPrimary',
  style,
  ...rest
}: AppTextProps) {
  const theme = useTheme();

  return (
    <Text
      {...rest}
      style={[theme.typography[variant], {color: theme.colors[color]}, style]}
    />
  );
}
