import React from 'react';
import {StatusBar} from 'react-native';
import {AppNavigator} from './navigation/AppNavigator';
import {AppProviders} from './providers/AppProviders';

export default function App(): React.JSX.Element {
  return (
    <AppProviders>
      <StatusBar barStyle="light-content" />
      <AppNavigator />
    </AppProviders>
  );
}
