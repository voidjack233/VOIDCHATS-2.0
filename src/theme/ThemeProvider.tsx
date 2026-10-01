import React, {createContext, useContext} from 'react';
import type {ReactNode} from 'react';
import {darkTheme} from './tokens';
import type {Theme} from './tokens';

const ThemeContext = createContext<Theme>(darkTheme);

export interface ThemeProviderProps {
  children: ReactNode;
  theme?: Theme;
}

export function ThemeProvider({children, theme = darkTheme}: ThemeProviderProps) {
  return <ThemeContext.Provider value={theme}>{children}</ThemeContext.Provider>;
}

export function useTheme(): Theme {
  return useContext(ThemeContext);
}
