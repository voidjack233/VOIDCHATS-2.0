import React, {useEffect, useState} from 'react';
import {ActivityIndicator, BackHandler, StyleSheet, View} from 'react-native';
import {SafeAreaView} from 'react-native-safe-area-context';
import {ChatScreen} from '../../chat';
import {ContactsScreen} from '../../contacts';
import {NewMessageScreen} from '../../contacts/NewMessageScreen';
import {ConversationsScreen, type Conversation} from '../../conversations';
import {RequestsScreen} from '../../requests';
import {SettingsScreen} from '../../settings';
import {useTheme} from '../../theme';
import {useAllConversations, useStorage} from '../../storage';
import {useMessaging} from '../../messaging';
import {AppText, IconButton} from '../../ui';
import {BottomTabBar, type MainTab} from './BottomTabBar';
import {StartupNavigator} from './StartupNavigator';
  
type Detail =
  | {name: 'chat'; conversation: Conversation; localDeviceId: string}
  | {name: 'newMessage'}
  | {name: 'settings'}
  | null;

const tabTitles: Record<MainTab, string> = {
  conversations: 'Messages',
  requests: 'Requests',
  contacts: 'Contacts',
};

function MainTabHeader({title, onOpenSettings}: {title: string; onOpenSettings: () => void}) {
  const {colors, spacing} = useTheme();
  const line = [styles.menuLine, {backgroundColor: colors.textPrimary}];
  return (
    <View style={[styles.header, {
      borderBottomColor: colors.border,
      gap: spacing.sm,
      paddingHorizontal: spacing.xl,
      paddingVertical: spacing.md,
    }]}>
      <AppText variant="display" accessibilityRole="header" style={styles.headerTitle}>{title}</AppText>
      <IconButton
        accessibilityLabel="Open settings"
        onPress={onOpenSettings}
        size={48}
        icon={
          <View style={styles.menuIcon}>
            <View style={line} />
            <View style={line} />
            <View style={line} />
          </View>
        }
      />
    </View>
  );
}

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
  const messaging = useMessaging();
  const conversations = useAllConversations();
  const requestIds = new Set(messaging.sessions.filter(session => session.isRequest).map(session => session.id));
  const blocked = new Set(messaging.blockedVoidIds);
  const chatCount = conversations.items.filter(item => !requestIds.has(item.id) && (!item.peerVoidId || !blocked.has(item.peerVoidId)))
    .reduce((total, item) => total + item.unreadCount, 0);
  const requestCount = messaging.sessions.filter(session => session.isRequest && session.hasMessages && !blocked.has(session.peer.void_id)).length;
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
  } else if (detail?.name === 'newMessage') {
    content = <NewMessageScreen onBack={() => setDetail(null)} onOpenConversation={openConversation} />;
  } else if (detail?.name === 'settings') {
    content = <SettingsScreen onBack={() => setDetail(null)} />;
  } else if (tab === 'requests') {
    content = <RequestsScreen onOpenConversation={openConversation} />;
  } else if (tab === 'contacts') {
    content = <ContactsScreen onOpenConversation={openConversation} />;
  } else {
    content = (
      <ConversationsScreen
        onOpenRequests={() => showTab('requests')}
        onNewMessage={() => setDetail({name: 'newMessage'})}
        onOpenConversation={openConversation}
      />
    );
  }

  return (
    <>
      {!detail && (
        <MainTabHeader
          title={tabTitles[tab] ?? tabTitles.conversations}
          onOpenSettings={() => setDetail({name: 'settings'})}
        />
      )}
      {content}
      {!detail && <BottomTabBar selected={tab} onSelect={showTab} chatCount={chatCount} requestCount={requestCount} />}
    </>
  );
}

const styles = StyleSheet.create({
  container: {flex: 1},
  header: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth},
  headerTitle: {flex: 1},
  menuIcon: {gap: 4},
  menuLine: {width: 20, height: 2, borderRadius: 1},
  restoring: {flex: 1, alignItems: 'center', justifyContent: 'center', padding: 24},
});
