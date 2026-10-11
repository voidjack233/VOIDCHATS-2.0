import React, {useEffect, useRef, useState} from 'react';
import {StyleSheet, View} from 'react-native';
import {useTheme} from '../theme';
import {AppText, Button, Surface, TextInput} from '../ui';
import {getDevelopmentBackendConfig, setDevelopmentBackendBaseURL} from './config';
import type {BackendConfig} from './config';
import {checkBackendHealth} from './health';
import type {BackendHealthResult} from './health';

export function BackendDiagnostic() {
  const theme = useTheme();
  const [baseURL, setBaseURL] = useState(getDevelopmentBackendConfig()?.baseURL ?? '');
  const [result, setResult] = useState<BackendHealthResult | null>(null);
  const [checking, setChecking] = useState(false);
  const request = useRef<AbortController | null>(null);

  useEffect(() => () => request.current?.abort(), []);

  async function checkConnection() {
    if (request.current) {
      return;
    }
    let config: BackendConfig;
    try {
      config = setDevelopmentBackendBaseURL(baseURL);
      setBaseURL(config.baseURL);
    } catch (error) {
      setResult({ok: false, message: error instanceof Error ? error.message : 'Enter a valid server address.'});
      return;
    }

    const controller = new AbortController();
    request.current = controller;
    setResult(null);
    setChecking(true);
    const checked = await checkBackendHealth(config, controller.signal);
    if (!controller.signal.aborted) {
      setResult(checked);
      setChecking(false);
      request.current = null;
    }
  }

  return (
    <View style={{marginTop: theme.spacing.xxl}}>
      <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, {marginBottom: theme.spacing.sm}]}>
        DEVELOPMENT
      </AppText>
      <Surface bordered padded>
        <AppText variant="heading">Backend connection</AppText>
        <AppText variant="caption" color="textSecondary" style={{marginTop: theme.spacing.sm, marginBottom: theme.spacing.lg}}>
          Use your computer’s LAN address or an HTTPS development tunnel. Localhost on a phone refers to the phone.
        </AppText>
        <TextInput
          label="Backend URL"
          value={baseURL}
          onChangeText={value => {
            setBaseURL(value);
            setResult(null);
          }}
          placeholder="http://host-lan-ip:8080"
          keyboardType="url"
          autoCapitalize="none"
          autoCorrect={false}
          editable={!checking}
          maxLength={2048}
        />
        <Button title="Check connection" variant="secondary" loading={checking} disabled={!baseURL.trim()} onPress={checkConnection} style={{marginTop: theme.spacing.md}} />
        {result ? (
          <AppText variant="caption" color={result.ok ? 'success' : 'danger'} accessibilityLiveRegion="polite" style={{marginTop: theme.spacing.md}}>
            {result.message}
          </AppText>
        ) : null}
      </Surface>
    </View>
  );
}

const styles = StyleSheet.create({
  sectionLabel: {letterSpacing: 1},
});
