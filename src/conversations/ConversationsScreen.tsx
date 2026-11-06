import React, {useEffect, useState} from 'react';
import {FlatList, Pressable, StyleSheet, View} from 'react-native';
import {useMessaging} from '../messaging';
import {useConversations, useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button, Divider, Screen} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';
import {ConversationRow} from './ConversationRow';
import {conversationFromRecord, type Conversation} from './types';

export interface ConversationsScreenProps {
  onOpenRequests: () => void;
  onNewMessage: () => void;
  onOpenConversation: (conversation: Conversation) => void;
}

const composeSize = 56;
const connectingNoticeDelay = 500;

function ConnectionNotice({status, error}: Pick<ReturnType<typeof useMessaging>, 'status' | 'error'>) {
  const {spacing} = useTheme();
  const [showConnecting, setShowConnecting] = useState(false);
  const connecting = status === 'connecting' && !error;

  useEffect(() => {
    if (!connecting) {
      setShowConnecting(false);
      return;
    }
    const timeout = setTimeout(() => setShowConnecting(true), connectingNoticeDelay);
    return () => clearTimeout(timeout);
  }, [connecting]);

  const message = error ?? (status === 'offline' ? 'You’re offline.' : connecting && showConnecting ? 'Connecting…' : null);
  return message ? (
    <View style={{paddingVertical: spacing.sm}}>
      <AppText variant="caption" color={error ? 'warning' : 'textSecondary'} accessibilityLiveRegion="polite">
        {message}
      </AppText>
    </View>
  ) : null;
}

export function ConversationsScreen({onOpenRequests, onNewMessage, onOpenConversation}: ConversationsScreenProps) {
  const {colors, radii, spacing} = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const conversations = useConversations();
  const registered = storage.status === 'ready' && Boolean(storage.identity?.accountVoidId);
  const sessionsById = new Map(messaging.sessions.map(session => [session.id, session]));
  const blocked = new Set(messaging.blockedVoidIds);
  const items = conversations.items.filter(item => {
    const session = sessionsById.get(item.id);
    return !session?.isRequest && !(session?.direction === 'incoming' && item.lastMessage === null);
  });
  const requestCount = messaging.sessions.filter(session =>
    session.isRequest && session.hasMessages && !blocked.has(session.peer.void_id)).length;
  const showEmptyState = conversations.hasLoaded && !conversations.error && !conversations.hasMore;

  return <Screen>
    <FlatList
      accessibilityState={{busy: storage.status === 'ready' && Boolean(storage.identity?.localDeviceId) &&
        !conversations.hasLoaded && !conversations.error}}
      data={items}
      keyExtractor={item => item.id}
      contentContainerStyle={{
        paddingHorizontal: spacing.xl,
        paddingBottom: composeSize + spacing.lg + spacing.md,
      }}
      ItemSeparatorComponent={Divider}
      ListHeaderComponent={<>
        <StorageStatus />
        {requestCount > 0 ? <Pressable accessibilityRole="button" accessibilityLabel={`Requests, ${requestCount} chats`} onPress={onOpenRequests}
          style={({pressed}) => [styles.requests, {
            paddingVertical: spacing.sm,
            marginBottom: spacing.sm,
            borderBottomColor: colors.border,
            backgroundColor: pressed ? colors.surface : 'transparent',
          }]}>
          <AppText style={styles.flex}>Requests</AppText>
          <View style={[styles.badge, {
            backgroundColor: colors.accent,
            borderRadius: radii.full,
            marginHorizontal: spacing.sm,
            paddingHorizontal: spacing.sm,
            paddingVertical: spacing.xs,
          }]}>
            <AppText variant="caption" color="accentText" numberOfLines={1}>
              {requestCount > 99 ? '99+' : requestCount}
            </AppText>
          </View>
          <AppText color="textMuted">›</AppText>
        </Pressable> : null}
        {registered ? <ConnectionNotice status={messaging.status} error={messaging.error} /> : null}
        {conversations.error ? <View style={{paddingVertical: spacing.lg}}><AppText color="danger">Could not load messages.</AppText><Button title="Try again" variant="ghost" onPress={conversations.refresh} /></View> : null}
      </>}
      renderItem={({item}) => {
        const conversation = conversationFromRecord(item);
        if (item.peerVoidId && item.title === `VOID-${item.peerVoidId}`) { conversation.name = item.peerVoidId; }
        if (!item.lastMessage) { conversation.preview = 'Write a message'; }
        return <ConversationRow conversation={conversation} onPress={() => onOpenConversation(conversation)} />;
      }}
      ListEmptyComponent={showEmptyState ? <View style={{paddingVertical: spacing.xxxl}}>
        <AppText variant="heading" accessibilityRole="header">No conversations yet</AppText>
        <AppText color="textSecondary" style={{marginTop: spacing.sm}}>
          Tap + to start a conversation with someone’s VOID-ID.
        </AppText>
      </View> : undefined}
      ListFooterComponent={conversations.hasMore ? <Button title="Load more" variant="ghost" loading={conversations.loading} onPress={() => { conversations.loadMore(); }} style={{marginTop: spacing.lg}} /> : undefined}
    />
    <Pressable accessibilityRole="button" accessibilityLabel="New message" onPress={onNewMessage}
      style={({pressed}) => [styles.compose, {
        end: spacing.xl,
        bottom: spacing.lg,
        backgroundColor: colors.accent,
        borderRadius: radii.xl,
        opacity: pressed ? 0.78 : 1,
      }]}>
      <View style={styles.composeIcon}>
        <View style={[styles.composeStroke, styles.horizontalStroke, {backgroundColor: colors.accentText}]} />
        <View style={[styles.composeStroke, styles.verticalStroke, {backgroundColor: colors.accentText}]} />
      </View>
    </Pressable>
  </Screen>;
}

const styles = StyleSheet.create({
  flex: {flex: 1},
  requests: {flexDirection: 'row', alignItems: 'center', minHeight: 48, borderBottomWidth: StyleSheet.hairlineWidth},
  badge: {minWidth: 24, minHeight: 24, alignItems: 'center', justifyContent: 'center'},
  compose: {
    position: 'absolute',
    width: composeSize,
    height: composeSize,
    alignItems: 'center',
    justifyContent: 'center',
    elevation: 2,
  },
  composeIcon: {width: 20, height: 20},
  composeStroke: {position: 'absolute', borderRadius: 1},
  horizontalStroke: {width: 20, height: 2, top: 9},
  verticalStroke: {width: 2, height: 20, left: 9},
});
