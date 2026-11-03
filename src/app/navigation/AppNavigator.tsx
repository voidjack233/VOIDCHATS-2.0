import React, {useEffect, useState} from 'react';
import {ActivityIndicator, BackHandler, StyleSheet, View} from 'react-native';
import {SafeAreaView} from 'react-native-safe-area-context';
import {ChatScreen} from '../../chat';
import {ContactsScreen} from '../../contacts';
import {ConversationsScreen, type Conversation} from '../../conversations';
import {ProfileScreen} from '../../profile';
import {RequestsScreen} from '../../requests';
import {SettingsScreen} from '../../settings';
import {useTheme} from '../../theme';
import {useStorage} from '../../storage';
import {AppText} from '../../ui';
import {BottomTabBar, type MainTab} from './BottomTabBar';
import {StartupNavigator} from './StartupNavigator';

type Detail =
  | {name: 'chat'; conversation: Conversation; localDeviceId: string}
  | {name: 'settings'}
  | null;

export function AppNavigator(): React.JSX.Element {
  const theme = useTheme();
  const storage = useStorage();
  const [mode, setMode] = useState<'startup' | 'preview'>('startup');
  const [restored, setRestored] = useState(false);
  const registered = storage.status === 'ready' && Boolean(storage.identity?.accountVoidId);

  useEffect(() => {
    if (storage.status !== 'opening') { setRestored(true); }
  }, [storage.status]);

  return (
    <SafeAreaView
      style={[styles.container, {backgroundColor: theme.colors.background}]}>
      {!restored && storage.status === 'opening' ? (
        <View style={styles.restoring}>
          <ActivityIndicator color={theme.colors.accent} />
          <AppText color="textSecondary" accessibilityLiveRegion="polite" style={{marginTop: theme.spacing.lg}}>
            Opening your encrypted storage…
          </AppText>
        </View>
      ) : !registered && mode === 'startup' ? (
        <StartupNavigator
          onPreview={() => {
            if (__DEV__) {
              setMode('preview');
            }
          }}
        />
      ) : (
        <MessengerNavigator />
      )}
    </SafeAreaView>
  );
}

function MessengerNavigator(): React.JSX.Element {
  const storage = useStorage();
  const [tab, setTab] = useState<MainTab>('conversations');
  const [detail, setDetail] = useState<Detail>(null);

  const localDeviceId = storage.status === 'ready' ? storage.identity?.localDeviceId : undefined;
  useEffect(() => {
    if (detail?.name === 'chat' && detail.localDeviceId !== localDeviceId) {
      setDetail(null);
    }
  }, [detail, localDeviceId]);

  useEffect(() => {
    const subscription = BackHandler.addEventListener('hardwareBackPress', () => {
      if (detail) {
        setDetail(null);
        return true;
      }
      if (tab !== 'conversations') {
        setTab('conversations');
        return true;
      }
      return false;
    });
    return () => subscription.remove();
  }, [detail, tab]);

  const showTab = (nextTab: MainTab) => {
    setDetail(null);
    setTab(nextTab);
  };
  const openConversation = (conversation: Conversation) => {
    if (localDeviceId) { setDetail({name: 'chat', conversation, localDeviceId}); }
  };

  let content: React.JSX.Element;
  if (detail?.name === 'chat' && localDeviceId && detail.localDeviceId === localDeviceId) {
    content = (
      <ChatScreen key={detail.conversation.id} conversation={detail.conversation} onBack={() => setDetail(null)} />
    );
  } else if (detail?.name === 'settings') {
    content = <SettingsScreen onBack={() => setDetail(null)} />;
  } else if (tab === 'requests') {
    content = <RequestsScreen onOpenConversation={openConversation} />;
  } else if (tab === 'contacts') {
    content = <ContactsScreen onOpenConversation={openConversation} />;
  } else if (tab === 'profile') {
    content = <ProfileScreen onOpenSettings={() => setDetail({name: 'settings'})} />;
  } else {
    content = (
      <ConversationsScreen
        onOpenRequests={() => showTab('requests')}
        onOpenContacts={() => showTab('contacts')}
        onOpenConversation={openConversation}
      />
    );
  }

  return (
    <>
      {content}
      {!detail && <BottomTabBar selected={tab} onSelect={showTab} />}
    </>
  );
}

const styles = StyleSheet.create({
  container: {flex: 1},
  restoring: {flex: 1, alignItems: 'center', justifyContent: 'center', padding: 24},
});
