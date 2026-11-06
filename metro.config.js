const { getDefaultConfig } = require('expo/metro-config');

const config = getDefaultConfig(__dirname);

// Skip the directory itself so Metro never watches Gradle's temporary sockets.
config.resolver.blockList = [
  ...config.resolver.blockList,
  /(?:^|[\\/])android[\\/]build(?:[\\/]|$)/,
];

module.exports = config;
