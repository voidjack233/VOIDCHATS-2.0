import React, {useEffect, useState} from 'react';
import {Image, StyleSheet, View} from 'react-native';
import type {ImageSourcePropType, StyleProp, ViewStyle} from 'react-native';
import {useTheme} from '../theme';
import {AppText} from './AppText';

export interface AvatarProps {
  name: string;
  size?: number;
  uri?: string;
  source?: ImageSourcePropType;
  style?: StyleProp<ViewStyle>;
}

function getInitials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length > 1) {
    return `${Array.from(words[0])[0]}${Array.from(words[words.length - 1])[0]}`.toUpperCase();
  }
  return Array.from(words[0] ?? '').slice(0, 2).join('').toUpperCase() || '?';
}

export function Avatar({name, size = 44, uri, source, style}: AvatarProps) {
  const {colors, radii} = useTheme();
  const [imageFailed, setImageFailed] = useState(false);
  const imageSource = source ?? (uri ? {uri} : undefined);

  useEffect(() => {
    setImageFailed(false);
  }, [uri, source]);

  return (
    <View
      accessible
      accessibilityRole="image"
      accessibilityLabel={`${name} avatar`}
      style={[
        styles.container,
        {
          width: size,
          height: size,
          borderRadius: radii.full,
          backgroundColor: colors.surfaceElevated,
        },
        style,
      ]}>
      {imageSource && !imageFailed ? (
        <Image
          source={imageSource}
          onError={() => setImageFailed(true)}
          style={{width: size, height: size}}
        />
      ) : (
        <AppText
          color="accent"
          variant="label"
          style={{fontSize: Math.max(12, Math.round(size * 0.34))}}>
          {getInitials(name)}
        </AppText>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    justifyContent: 'center',
    overflow: 'hidden',
  },
});
