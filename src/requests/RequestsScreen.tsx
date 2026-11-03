import React from 'react';
import {FlatList, View} from 'react-native';
import {ConversationRow} from '../conversations/ConversationRow';
import {conversationFromRecord, type Conversation} from '../conversations/types';
import {useMessaging} from '../messaging';
import {safeMessagingError} from '../messaging/errors';
import {useConversations, useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button, Divider, Screen} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export interface RequestsScreenProps {onOpenConversation: (conversation: Conversation) => void}

export function RequestsScreen({onOpenConversation}: RequestsScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const conversations = useConversations();
  const requestIds = new Set(messaging.sessions.filter(session => session.isRequest).map(session => session.id));
  const items = conversations.items.filter(item => requestIds.has(item.id) && item.lastMessage !== null);
  const incomingIssue = messaging.sessions.find(session => session.direction === 'incoming' && session.diagnosticCode);
  const issueMessage = incomingIssue ? safeMessagingError({code: incomingIssue.diagnosticCode}).message : messaging.error;

  return <Screen>
    <FlatList
      data={items}
      keyExtractor={item => item.id}
      refreshing={conversations.loading && storage.status === 'ready'}
      onRefresh={() => { conversations.refresh(); messaging.refresh().catch(() => {}); }}
      contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl}}
      ItemSeparatorComponent={Divider}
      ListHeaderComponent={<View style={{paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xl}}>
        <AppText variant="title">Requests</AppText>
        <AppText color="textSecondary" style={{marginTop: theme.spacing.sm}}>Messages from people you haven’t replied to.</AppText>
        <StorageStatus />
        {issueMessage ? <View style={{marginTop: theme.spacing.lg}}>
          <AppText variant="caption" color="warning" accessibilityLiveRegion="polite">{incomingIssue ? 'An incoming message could not be processed. ' : ''}{issueMessage}</AppText>
          {incomingIssue?.diagnosticCode ? <AppText variant="caption" color="textMuted" selectable style={{marginTop: theme.spacing.xs}}>Error code: {incomingIssue.diagnosticCode}</AppText> : null}
        </View> : null}
        {conversations.error ? <View style={{marginTop: theme.spacing.lg}}><AppText color="danger">Could not load requests.</AppText><Button title="Try again" variant="ghost" onPress={conversations.refresh} /></View> : null}
      </View>}
      renderItem={({item}) => {
        const conversation = conversationFromRecord(item);
        if (item.peerVoidId && item.title === `VOID-${item.peerVoidId}`) { conversation.name = item.peerVoidId; }
        return <ConversationRow conversation={conversation} onPress={() => onOpenConversation(conversation)} />;
      }}
      ListEmptyComponent={storage.status === 'ready' && !conversations.error ? <View style={{paddingVertical: theme.spacing.xxxl}}>
        <AppText variant="heading">{conversations.loading ? 'Loading requests…' : 'No requests'}</AppText>
        <AppText color="textSecondary" style={{marginTop: theme.spacing.sm}}>First messages from new people will appear here. Open one to read or reply.</AppText>
      </View> : undefined}
      ListFooterComponent={conversations.hasMore ? <Button title="Load more" variant="ghost" loading={conversations.loading} onPress={() => { conversations.loadMore(); }} style={{marginTop: theme.spacing.lg}} /> : undefined}
    />
  </Screen>;
}
