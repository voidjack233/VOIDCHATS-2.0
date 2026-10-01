import type {TextStyle} from 'react-native';

export type ThemeColor =
  | 'background'
  | 'surface'
  | 'surfaceElevated'
  | 'textPrimary'
  | 'textSecondary'
  | 'textMuted'
  | 'border'
  | 'accent'
  | 'accentText'
  | 'danger'
  | 'warning'
  | 'success';

export type TextVariant =
  | 'display'
  | 'title'
  | 'heading'
  | 'body'
  | 'label'
  | 'caption';

export interface Theme {
  mode: 'dark' | 'light';
  colors: Record<ThemeColor, string>;
  spacing: {
    xs: number;
    sm: number;
    md: number;
    lg: number;
    xl: number;
    xxl: number;
    xxxl: number;
  };
  radii: {
    sm: number;
    md: number;
    lg: number;
    xl: number;
    full: number;
  };
  typography: Record<TextVariant, TextStyle>;
}

export const darkTheme: Theme = {
  mode: 'dark',
  colors: {
    background: '#0C1118',
    surface: '#151C25',
    surfaceElevated: '#1D2732',
    textPrimary: '#F4F6F8',
    textSecondary: '#B0BAC5',
    textMuted: '#7C8997',
    border: '#2D3946',
    accent: '#95AFE9',
    accentText: '#101826',
    danger: '#E8868C',
    warning: '#E1BA79',
    success: '#88C8A8',
  },
  spacing: {
    xs: 4,
    sm: 8,
    md: 12,
    lg: 16,
    xl: 24,
    xxl: 32,
    xxxl: 40,
  },
  radii: {
    sm: 6,
    md: 10,
    lg: 14,
    xl: 20,
    full: 999,
  },
  typography: {
    display: {fontSize: 32, lineHeight: 38, fontWeight: '700'},
    title: {fontSize: 24, lineHeight: 30, fontWeight: '700'},
    heading: {fontSize: 19, lineHeight: 25, fontWeight: '600'},
    body: {fontSize: 16, lineHeight: 23, fontWeight: '400'},
    label: {fontSize: 14, lineHeight: 20, fontWeight: '600'},
    caption: {fontSize: 13, lineHeight: 18, fontWeight: '400'},
  },
};
