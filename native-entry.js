/** @format */

import {AppRegistry} from 'react-native';
import App from './src/app/App';

// The custom Android build uses the existing bare native project.
AppRegistry.registerComponent('VoidChats', () => App);

if (__DEV__) {
  // Invoke from the React Native DevTools console on an actual Android phone.
  // Lazy loading keeps normal startup free of account/key initialization.
  global.voidMlsSelfTest = () => require('./src/crypto').openMls.runSelfTest();
}
