import React, {forwardRef} from 'react';
import {StyleSheet, TextInput as NativeTextInput, View} from 'react-native';
import type {StyleProp, TextInputProps as NativeTextInputProps, ViewStyle} from 'react-native';
import {useTheme} from '../theme';
import {AppText} from './AppText';

export interface TextInputProps extends NativeTextInputProps {
  label?: string;
  error?: string;
  containerStyle?: StyleProp<ViewStyle>;
}

export const TextInput = forwardRef<React.ElementRef<typeof NativeTextInput>, TextInputProps>(
  function ForwardedTextInput({label, error, containerStyle, style, ...rest}, ref) {
    const {colors, mode, radii, spacing, typography} = useTheme();

    return (
      <View style={containerStyle}>
        {label ? (
          <AppText variant="label" style={{marginBottom: spacing.sm}}>
            {label}
          </AppText>
        ) : null}
        <NativeTextInput
          {...rest}
          ref={ref}
          accessibilityLabel={rest.accessibilityLabel ?? label}
          accessibilityHint={rest.accessibilityHint ?? error}
          keyboardAppearance={rest.keyboardAppearance ?? mode}
          placeholderTextColor={rest.placeholderTextColor ?? colors.textMuted}
          selectionColor={rest.selectionColor ?? colors.accent}
          style={[
            typography.body,
            styles.input,
            {
              backgroundColor: colors.surface,
              borderColor: error ? colors.danger : colors.border,
              borderRadius: radii.md,
              color: colors.textPrimary,
              paddingHorizontal: spacing.lg,
              paddingVertical: spacing.md,
            },
            style,
          ]}
        />
        {error ? (
          <AppText color="danger" variant="caption" style={{marginTop: spacing.xs}}>
            {error}
          </AppText>
        ) : null}
      </View>
    );
  },
);

const styles = StyleSheet.create({
  input: {
    borderWidth: 1,
    minHeight: 48,
  },
});
