import {NativeModules, Platform} from 'react-native';
import {createStorageDriver} from './bridge';
import {decodeSnapshot} from './decode';
import type {NativeStorageModule} from './types';

export const nativeStorage = createStorageDriver(
  () => NativeModules.VoidStorage as NativeStorageModule | undefined,
  Platform.OS,
  __DEV__,
  decodeSnapshot,
);
