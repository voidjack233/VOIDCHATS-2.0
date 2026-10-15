import React, {useEffect, useState} from 'react';
import {BackHandler, StyleSheet} from 'react-native';
import {SafeAreaView} from 'react-native-safe-area-context';
import {ChatScreen} from '../../chat';
import {ContactsScreen} from '../../contacts';
import {ConversationsScreen, type Conversation} from '../../conversations';
import {ProfileScreen} from '../../profile';
import {RequestsScreen} from '../../requests';
import {SettingsScreen} from '../../settings';
import {useTheme} from '../../theme';
import {useStorage} from '../../storage';
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

  useEffect(() => {
    // Restoring a local development identity does not register a VOID account.
    if (__DEV__ && storage.status === 'ready' && storage.identity) {
      setMode('preview');
    }
  }, [storage.status, storage.identity]);

  return (
    <SafeAreaView
      style={[styles.container, {backgroundColor: theme.colors.background}]}>
      {mode === 'startup' ? (
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

  let content: React.JSX.Element;
  if (detail?.name === 'chat' && localDeviceId && detail.localDeviceId === localDeviceId) {
    content = (
      <ChatScreen conversation={detail.conversation} onBack={() => setDetail(null)} />
    );
  } else if (detail?.name === 'settings') {
    content = <SettingsScreen onBack={() => setDetail(null)} />;
  } else if (tab === 'requests') {
    content = <RequestsScreen />;
  } else if (tab === 'contacts') {
    content = <ContactsScreen />;
  } else if (tab === 'profile') {
    content = <ProfileScreen onOpenSettings={() => setDetail({name: 'settings'})} />;
  } else {
    content = (
      <ConversationsScreen
        onOpenRequests={() => showTab('requests')}
        onOpenContacts={() => showTab('contacts')}
        onOpenConversation={conversation => {
          if (localDeviceId) { setDetail({name: 'chat', conversation, localDeviceId}); }
        }}
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
});
