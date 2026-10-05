import React from 'react';
import { ScrollView, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Screen } from '../ui';

export function RequestsScreen() {
  const theme = useTheme();

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}>
        <AppText variant="title">Message requests</AppText>
        <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
          Messages from people outside your contacts appear here.
        </AppText>
        <View style={{ marginTop: theme.spacing.xxxl }}>
          <AppText variant="heading">No message requests</AppText>
          <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
            Incoming requests will appear here once messaging is connected.
          </AppText>
        </View>
      </ScrollView>
    </Screen>
  );
}
