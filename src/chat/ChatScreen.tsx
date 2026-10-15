import React, {useEffect, useMemo, useRef} from 'react';
import {FlatList, StyleSheet, View} from 'react-native';
import type {Conversation} from '../conversations';
import {useMessages, useStorage} from '../storage';
import type {MessageRecord} from '../storage/types';
import {useTheme} from '../theme';
import {AppText, Avatar, Button, IconButton, Screen, TextInput} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export interface ChatScreenProps {
  conversation: Conversation;
  onBack: () => void;
}

function MessageRow({message}: {message: MessageRecord}) {
  const theme = useTheme();
  const outgoing = message.direction === 'outgoing';
  const reactions = new Map<string, number>();
  for (const reaction of message.reactions) {
    reactions.set(reaction.emoji, (reactions.get(reaction.emoji) ?? 0) + 1);
  }
  return (
    <View style={[styles.message, outgoing ? styles.outgoing : styles.incoming, {backgroundColor: outgoing ? theme.colors.surfaceElevated : theme.colors.surface, borderRadius: theme.radii.lg, padding: theme.spacing.md, marginBottom: theme.spacing.md}]}>
      {!outgoing ? <AppText variant="caption" color="textMuted" numberOfLines={1}>{message.senderVoidId}</AppText> : null}
      <AppText variant="body" color={message.deletedAt !== null ? 'textMuted' : 'textPrimary'} style={!outgoing ? {marginTop: theme.spacing.xs} : undefined}>
        {message.deletedAt !== null ? 'Message deleted' : message.body}
      </AppText>
      {reactions.size > 0 ? <AppText variant="caption" color="textSecondary" style={{marginTop: theme.spacing.sm}}>{Array.from(reactions, ([emoji, count]) => emoji + ' ' + count).join('  ')}</AppText> : null}
      <AppText variant="caption" color="textMuted" style={{marginTop: theme.spacing.sm}}>
        {new Date(message.createdAt).toLocaleString()}
        {message.editedAt !== null && message.deletedAt === null ? ' · Edited' : ''}
        {outgoing ? ' · ' + message.deliveryState : ''}
      </AppText>
    </View>
  );
}

export function ChatScreen({conversation, onBack}: ChatScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const messages = useMessages(conversation.id);
  const list = useRef<FlatList<MessageRecord>>(null);
  const initiallyScrolled = useRef(false);
  const readBatch = useRef('');
  const chronological = useMemo(() => [...messages.items].reverse(), [messages.items]);

  useEffect(() => {
    initiallyScrolled.current = false;
    readBatch.current = '';
  }, [conversation.id]);

  useEffect(() => {
    if (storage.status !== 'ready' || messages.loading || messages.error) { return; }
    const unread = messages.items.filter(message => message.direction === 'incoming' && message.readAt === null && message.deletedAt === null);
    const batch = unread.map(message => message.id).sort().join('\n');
    if (!batch || batch === readBatch.current) { return; }
    readBatch.current = batch;
    const newestUnread = unread[0];
    storage.repositories.conversations.markRead(conversation.id, {createdAt: newestUnread.createdAt, id: newestUnread.id}).catch(() => { readBatch.current = ''; });
  }, [conversation.id, messages.items, messages.loading, messages.error, storage.status, storage.repositories]);

  return (
    <Screen>
      <View style={[styles.header, {paddingHorizontal: theme.spacing.md, paddingVertical: theme.spacing.sm, borderBottomColor: theme.colors.border}]}>
        <IconButton accessibilityLabel="Back to conversations" onPress={onBack} icon={<AppText variant="heading">‹</AppText>} style={{marginRight: theme.spacing.xs}} />
        <Avatar name={conversation.name} size={40} />
        <View style={[styles.flex, {marginLeft: theme.spacing.md}]}>
          <AppText variant="label" numberOfLines={1}>{conversation.name}</AppText>
          <AppText variant="caption" color="textSecondary" numberOfLines={1}>{conversation.kind === 'group' ? 'Group conversation' : conversation.voidId ?? 'Direct conversation'}</AppText>
        </View>
      </View>
      <FlatList
        ref={list}
        data={chronological}
        keyExtractor={item => item.id}
        renderItem={({item}) => <MessageRow message={item} />}
        maintainVisibleContentPosition={{minIndexForVisible: 0}}
        contentContainerStyle={[styles.messages, {paddingHorizontal: theme.spacing.lg, paddingVertical: theme.spacing.xl}]}
        onContentSizeChange={() => {
          if (!initiallyScrolled.current && chronological.length > 0) {
            initiallyScrolled.current = true;
            list.current?.scrollToEnd({animated: false});
          }
        }}
        ListHeaderComponent={<>
          <StorageStatus />
          {messages.hasMore && storage.status === 'ready' ? <Button title="Load earlier messages" variant="secondary" loading={messages.loading} onPress={() => { messages.loadMore(); }} style={{marginBottom: theme.spacing.lg}} /> : null}
          {messages.error && storage.status === 'ready' ? <View style={{marginBottom: theme.spacing.lg}}><AppText variant="body" color="danger">Could not load messages.</AppText><Button title="Try again" variant="secondary" onPress={() => { messages.refresh(); }} style={{marginTop: theme.spacing.sm}} /></View> : null}
        </>}
        ListEmptyComponent={storage.status === 'ready' && !messages.error ? <View style={styles.empty}>
          <AppText variant="heading">{messages.loading ? 'Loading messages…' : 'No messages yet'}</AppText>
          <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>Messages stored on this device appear here.</AppText>
        </View> : null}
      />
      <View style={[styles.composer, {paddingHorizontal: theme.spacing.lg, paddingTop: theme.spacing.md, paddingBottom: theme.spacing.lg, borderTopColor: theme.colors.border}]}>
        <View style={styles.composerRow}>
          <TextInput accessibilityLabel="Message composer, sending unavailable" placeholder="Message" editable={false} containerStyle={styles.flex} style={styles.disabledInput} />
          <Button title="Send" disabled size="sm" style={{marginLeft: theme.spacing.sm}} />
        </View>
        <AppText variant="caption" color="textMuted" style={{marginTop: theme.spacing.sm}}>Sending is unavailable until encrypted delivery is connected.</AppText>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  header: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: 1},
  flex: {flex: 1},
  message: {maxWidth: '88%'},
  incoming: {alignSelf: 'flex-start'},
  outgoing: {alignSelf: 'flex-end'},
  messages: {flexGrow: 1},
  empty: {alignItems: 'center'},
  composer: {borderTopWidth: 1},
  composerRow: {flexDirection: 'row', alignItems: 'center'},
  disabledInput: {opacity: 0.6},
});
