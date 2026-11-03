import React from 'react';
import {SafeAreaProvider} from 'react-native-safe-area-context';
import {ThemeProvider} from '../../theme';
import {StorageProvider} from '../../storage';
import {AccountProvider} from '../../account';
import {MessagingProvider} from '../../messaging';

type AppProvidersProps = {
  children: React.ReactNode;
};

export function AppProviders({children}: AppProvidersProps): React.JSX.Element {
  return (
    <SafeAreaProvider>
      <ThemeProvider><StorageProvider><AccountProvider><MessagingProvider>{children}</MessagingProvider></AccountProvider></StorageProvider></ThemeProvider>
    </SafeAreaProvider>
  );
}
