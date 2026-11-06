import React from 'react';
import {FlatList, Pressable, StyleSheet, View} from 'react-native';
import {useMessaging} from '../messaging';
import {useConversations, useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button, Divider, Screen} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';
import {ConversationRow} from './ConversationRow';
import {conversationFromRecord, type Conversation} from './types';

export interface ConversationsScreenProps {
  onOpenRequests: () => void; onNewMessage: () => void;
  onOpenConversation: (conversation: Conversation) => void;
}

export function ConversationsScreen({onOpenRequests, onNewMessage, onOpenConversation}: ConversationsScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const conversations = useConversations();
  const registered = storage.status === 'ready' && Boolean(storage.identity?.accountVoidId);
  const requestSessions = messaging.sessions.filter(session => session.isRequest && !messaging.blockedVoidIds.includes(session.peer.void_id));
  const requestIds = new Set(messaging.sessions.filter(session => session.isRequest).map(session => session.id));
  const items = conversations.items.filter(item => !requestIds.has(item.id) &&
    !(messaging.sessions.find(session => session.id === item.id)?.direction === 'incoming' && item.lastMessage === null));
  const requestCount = requestSessions.filter(session => session.hasMessages).length;
  const connecting = messaging.status === 'connecting';

  return <Screen>
    <FlatList
      data={items}
      keyExtractor={item => item.id}
      contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: 96}}
      ItemSeparatorComponent={Divider}
      ListHeaderComponent={<>
        <StorageStatus />
        {requestCount > 0 ? <Pressable accessibilityRole="button" accessibilityLabel={`Requests, ${requestCount} chats`} onPress={onOpenRequests}
          style={({pressed}) => [styles.requests, {paddingVertical: theme.spacing.lg, marginBottom: theme.spacing.md, borderBottomColor: theme.colors.border, opacity: pressed ? 0.65 : 1}]}>
          <AppText variant="label" color="textSecondary" style={styles.flex}>Requests</AppText>
          {requestCount > 0 ? <View style={[styles.badge, {backgroundColor: theme.colors.accent, marginRight: theme.spacing.sm}]}><AppText variant="caption" color="accentText">{requestCount}</AppText></View> : null}
          <AppText color="textMuted">›</AppText>
        </Pressable> : null}
        {registered && (messaging.error || connecting || messaging.status === 'offline') ? <View style={{paddingBottom: theme.spacing.lg}}>
          <AppText variant="caption" color={messaging.error ? 'warning' : 'textMuted'} accessibilityLiveRegion="polite">{messaging.error ?? (connecting ? 'Connecting…' : 'You’re offline.')}</AppText>
        </View> : null}
        {conversations.error ? <View style={{paddingVertical: theme.spacing.lg}}><AppText color="danger">Could not load messages.</AppText><Button title="Try again" variant="ghost" onPress={conversations.refresh} /></View> : null}
      </>}
      renderItem={({item}) => {
        const conversation = conversationFromRecord(item);
        if (item.peerVoidId && item.title === `VOID-${item.peerVoidId}`) { conversation.name = item.peerVoidId; }
        if (!item.lastMessage) { conversation.preview = 'Write a message'; }
        return <ConversationRow conversation={conversation} onPress={() => onOpenConversation(conversation)} />;
      }}
      ListEmptyComponent={storage.status === 'ready' && !conversations.error ? <View style={{paddingVertical: theme.spacing.xxxl}}>
        <AppText variant="heading">{conversations.loading ? 'Loading messages…' : 'Your messages, right here'}</AppText>
        <AppText color="textSecondary" style={{marginTop: theme.spacing.sm}}>Start a conversation with someone’s VOID-ID.</AppText>
      </View> : undefined}
      ListFooterComponent={conversations.hasMore ? <Button title="Load more" variant="ghost" loading={conversations.loading} onPress={() => { conversations.loadMore(); }} style={{marginTop: theme.spacing.lg}} /> : undefined}
    />
    <Pressable accessibilityRole="button" accessibilityLabel="New message" onPress={onNewMessage}
      style={({pressed}) => [styles.compose, {backgroundColor: theme.colors.accent, opacity: pressed ? 0.7 : 1}]}>
      <AppText color="accentText" style={styles.plus}>+</AppText>
    </Pressable>
  </Screen>;
}
const styles = StyleSheet.create({flex: {flex: 1}, requests: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth}, badge: {borderRadius: 12, minWidth: 22, paddingHorizontal: 6, alignItems: 'center'}, compose: {position: 'absolute', right: 24, bottom: 24, width: 56, height: 56, borderRadius: 28, alignItems: 'center', justifyContent: 'center', elevation: 4}, plus: {fontSize: 32, lineHeight: 38}});
