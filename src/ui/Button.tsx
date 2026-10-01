import React from 'react';
import type {ReactNode} from 'react';
import {ActivityIndicator, Pressable, StyleSheet, View} from 'react-native';
import type {PressableProps, StyleProp, TextStyle, ViewStyle} from 'react-native';
import {useTheme} from '../theme';
import {AppText} from './AppText';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type ButtonSize = 'sm' | 'md';

export interface ButtonProps extends Omit<PressableProps, 'children' | 'style'> {
  title: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  leftIcon?: ReactNode;
  rightIcon?: ReactNode;
  fullWidth?: boolean;
  style?: StyleProp<ViewStyle>;
  textStyle?: StyleProp<TextStyle>;
}

export function Button({
  title,
  variant = 'primary',
  size = 'md',
  loading = false,
  leftIcon,
  rightIcon,
  fullWidth = false,
  disabled,
  style,
  textStyle,
  accessibilityLabel,
  ...rest
}: ButtonProps) {
  const {colors, radii, spacing} = useTheme();
  const inactive = disabled || loading;
  const foreground =
    variant === 'primary' ? colors.accentText :
    variant === 'danger' ? colors.background :
    variant === 'ghost' ? colors.accent : colors.textPrimary;
  const background =
    variant === 'primary' ? colors.accent :
    variant === 'danger' ? colors.danger :
    variant === 'secondary' ? colors.surfaceElevated : 'transparent';

  return (
    <Pressable
      {...rest}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel ?? title}
      accessibilityState={{disabled: inactive, busy: loading}}
      disabled={inactive}
      style={({pressed}) => [
        styles.button,
        {
          alignSelf: fullWidth ? 'stretch' : 'flex-start',
          backgroundColor: background,
          borderColor: variant === 'secondary' ? colors.border : 'transparent',
          borderRadius: radii.md,
          borderWidth: variant === 'secondary' ? 1 : 0,
          minHeight: size === 'sm' ? 38 : 48,
          opacity: inactive ? 0.5 : pressed ? 0.78 : 1,
          paddingHorizontal: size === 'sm' ? spacing.md : spacing.lg,
        },
        style,
      ]}>
      {loading ? (
        <ActivityIndicator color={foreground} size="small" />
      ) : (
        <>
          {leftIcon ? <View style={{marginRight: spacing.sm}}>{leftIcon}</View> : null}
          <AppText
            variant="label"
            style={[styles.label, {color: foreground}, textStyle]}>
            {title}
          </AppText>
          {rightIcon ? <View style={{marginLeft: spacing.sm}}>{rightIcon}</View> : null}
        </>
      )}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'center',
  },
  label: {
    textAlign: 'center',
  },
});
