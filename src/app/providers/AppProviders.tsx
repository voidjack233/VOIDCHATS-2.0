import React from 'react';
import {SafeAreaProvider} from 'react-native-safe-area-context';
import {ThemeProvider} from '../../theme';
import {StorageProvider} from '../../storage';

type AppProvidersProps = {
  children: React.ReactNode;
};

export function AppProviders({children}: AppProvidersProps): React.JSX.Element {
  return (
    <SafeAreaProvider>
      <ThemeProvider><StorageProvider>{children}</StorageProvider></ThemeProvider>
    </SafeAreaProvider>
  );
}
