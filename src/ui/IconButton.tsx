import React from 'react';
import type {ReactNode} from 'react';
import {Pressable} from 'react-native';
import type {PressableProps, StyleProp, ViewStyle} from 'react-native';
import {useTheme} from '../theme';

export interface IconButtonProps extends Omit<PressableProps, 'children' | 'style'> {
  icon: ReactNode;
  accessibilityLabel: string;
  size?: number;
  variant?: 'plain' | 'surface';
  style?: StyleProp<ViewStyle>;
}

export function IconButton({
  icon,
  accessibilityLabel,
  size = 44,
  variant = 'plain',
  disabled,
  style,
  ...rest
}: IconButtonProps) {
  const {colors, radii} = useTheme();

  return (
    <Pressable
      {...rest}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel}
      accessibilityState={{disabled: !!disabled}}
      disabled={disabled}
      hitSlop={size < 44 ? (44 - size) / 2 : 0}
      style={({pressed}) => [
        {
          alignItems: 'center',
          justifyContent: 'center',
          width: size,
          height: size,
          borderRadius: radii.full,
          backgroundColor: variant === 'surface' ? colors.surfaceElevated : 'transparent',
          opacity: disabled ? 0.5 : pressed ? 0.7 : 1,
        },
        style,
      ]}>
      {icon}
    </Pressable>
  );
}
