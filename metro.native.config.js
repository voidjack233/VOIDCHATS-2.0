const {getDefaultConfig, mergeConfig} = require('@react-native/metro-config');

// The custom native build uses RN's transformer/serializer together. Expo Go
// continues to use metro.config.js with Expo's matching transformer/serializer.
module.exports = mergeConfig(getDefaultConfig(__dirname), {
  resolver: {
    blockList: [
      /[/\\]native[/\\]openmls[/\\]target[/\\].*/,
      /[/\\]android[/\\](?:app[/\\])?build[/\\].*/,
    ],
  },
});
