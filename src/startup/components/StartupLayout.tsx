import React from 'react';
import {ScrollView, StyleSheet, View} from 'react-native';
import {useTheme} from '../../theme';
import {AppText, IconButton, Screen} from '../../ui';

interface StartupLayoutProps {
  children: React.ReactNode;
  footer?: React.ReactNode;
  onBack?: () => void;
}

export function StartupLayout({children, footer, onBack}: StartupLayoutProps) {
  const theme = useTheme();

  return (
    <Screen>
      <View
        style={[
          styles.toolbar,
          {paddingHorizontal: theme.spacing.md, paddingTop: theme.spacing.sm},
        ]}>
        {onBack ? (
          <IconButton
            accessibilityLabel="Go back"
            onPress={onBack}
            icon={<AppText variant="title">‹</AppText>}
          />
        ) : null}
      </View>
      <ScrollView
        contentContainerStyle={[
          styles.content,
          {
            paddingHorizontal: theme.spacing.xl,
            paddingTop: theme.spacing.xxl,
            paddingBottom: theme.spacing.xl,
          },
        ]}>
        {children}
      </ScrollView>
      {footer ? (
        <View
          style={{
            paddingHorizontal: theme.spacing.xl,
            paddingTop: theme.spacing.lg,
            paddingBottom: theme.spacing.xl,
          }}>
          {footer}
        </View>
      ) : null}
    </Screen>
  );
}

const styles = StyleSheet.create({
  toolbar: {minHeight: 52},
  content: {flexGrow: 1},
});
