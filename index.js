/**
 * @format
 */

import {registerRootComponent} from 'expo';
import {AppRegistry} from 'react-native';
import App from './src/app/App';

// Registers "main" for Expo Go.
registerRootComponent(App);

// Keeps the existing bare React Native Android/iOS projects runnable too.
AppRegistry.registerComponent('VoidChats', () => App);
