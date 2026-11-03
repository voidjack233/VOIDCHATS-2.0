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
  onOpenRequests: () => void; onOpenContacts: () => void;
  onOpenConversation: (conversation: Conversation) => void;
}

export function ConversationsScreen({onOpenRequests, onOpenContacts, onOpenConversation}: ConversationsScreenProps) {
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
      refreshing={conversations.loading && storage.status === 'ready'}
      onRefresh={() => { conversations.refresh(); if (registered) { messaging.refresh().catch(() => {}); } }}
      contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl}}
      ItemSeparatorComponent={Divider}
      ListHeaderComponent={<>
        <View style={[styles.heading, {paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.lg}]}>
          <AppText variant="title" style={styles.flex}>Messages</AppText>
          <Button title="New message" variant="ghost" size="sm" onPress={onOpenContacts} />
        </View>
        <StorageStatus />
        <Pressable accessibilityRole="button" accessibilityLabel={`Requests${requestCount ? `, ${requestCount} chats` : ''}`} onPress={onOpenRequests}
          style={({pressed}) => [styles.requests, {paddingVertical: theme.spacing.lg, marginBottom: theme.spacing.md, borderBottomColor: theme.colors.border, opacity: pressed ? 0.65 : 1}]}>
          <AppText variant="label" color="textSecondary" style={styles.flex}>Requests</AppText>
          {requestCount > 0 ? <View style={[styles.badge, {backgroundColor: theme.colors.accent, marginRight: theme.spacing.sm}]}><AppText variant="caption" color="accentText">{requestCount}</AppText></View> : null}
          <AppText color="textMuted">›</AppText>
        </Pressable>
        {registered && (messaging.error || connecting || messaging.status === 'offline') ? <View style={{paddingBottom: theme.spacing.lg}}>
          <AppText variant="caption" color={messaging.error ? 'warning' : 'textMuted'} accessibilityLiveRegion="polite">{messaging.error ?? (connecting ? 'Connecting…' : 'You’re offline. Messages will send when you reconnect.')}</AppText>
          {!connecting ? <Button title="Retry connection" variant="ghost" size="sm" onPress={() => { messaging.refresh().catch(() => {}); }} style={{marginTop: theme.spacing.xs}} /> : null}
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
        <AppText color="textSecondary" style={{marginTop: theme.spacing.sm}}>Start with someone’s VOID-ID.</AppText>
        <Button title="New message" onPress={onOpenContacts} style={{marginTop: theme.spacing.xl}} />
      </View> : undefined}
      ListFooterComponent={conversations.hasMore ? <Button title="Load more" variant="ghost" loading={conversations.loading} onPress={() => { conversations.loadMore(); }} style={{marginTop: theme.spacing.lg}} /> : undefined}
    />
  </Screen>;
}
const styles = StyleSheet.create({heading: {flexDirection: 'row', alignItems: 'center'}, flex: {flex: 1}, requests: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth}, badge: {borderRadius: 12, minWidth: 22, paddingHorizontal: 6, alignItems: 'center'}});
