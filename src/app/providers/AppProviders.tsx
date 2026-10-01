import React from 'react';
import {SafeAreaProvider} from 'react-native-safe-area-context';
import {ThemeProvider} from '../../theme';

type AppProvidersProps = {
  children: React.ReactNode;
};

export function AppProviders({children}: AppProvidersProps): React.JSX.Element {
  return (
    <SafeAreaProvider>
      <ThemeProvider>{children}</ThemeProvider>
    </SafeAreaProvider>
  );
}
