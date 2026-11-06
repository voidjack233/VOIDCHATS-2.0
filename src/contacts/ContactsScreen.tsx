import React from 'react';
import {FlatList, Pressable, StyleSheet, View} from 'react-native';
import {conversationFromRecord, type Conversation} from '../conversations/types';
import {useMessaging} from '../messaging';
import {useAllConversations, useContacts, useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Avatar, Button, Screen} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export interface ContactsScreenProps {onOpenConversation: (conversation: Conversation) => void}

export function ContactsScreen({onOpenConversation}: ContactsScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const conversations = useAllConversations();
  const contacts = useContacts();
  const requestIds = new Set(messaging.sessions.filter(session => session.isRequest).map(session => session.id));
  const seen = new Set<string>();
  const items = conversations.items.filter(item => {
    if (item.kind !== 'direct' || !item.peerVoidId || !item.lastMessage || requestIds.has(item.id) || seen.has(item.peerVoidId)) { return false; }
    seen.add(item.peerVoidId);
    return true;
  });

  return <Screen>
    <FlatList
      data={items}
      keyExtractor={item => item.id}
      contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxxl}}
      ListHeaderComponent={<>
        <StorageStatus />
        {conversations.error ? <View style={{marginTop: theme.spacing.lg}}><AppText color="danger">Could not load contacts.</AppText><Button title="Try again" variant="ghost" onPress={conversations.refresh} /></View> : null}
      </>}
      renderItem={({item}) => {
        const id = item.peerVoidId!;
        const contact = contacts.items.find(entry => entry.voidId === id);
        const name = contact?.displayName ?? item.title;
        const displayName = name === `VOID-${id}` ? id : name;
        const blocked = messaging.blockedVoidIds.includes(id);
        return <Pressable accessibilityRole="button" accessibilityLabel={`Message ${displayName}`} disabled={blocked}
          onPress={() => { const conversation = conversationFromRecord(item); conversation.name = displayName; onOpenConversation(conversation); }}
          style={({pressed}) => [styles.row, {paddingVertical: theme.spacing.lg, borderBottomColor: theme.colors.border, opacity: pressed || blocked ? 0.55 : 1}]}>
          <Avatar name={displayName} size={44} />
          <View style={[styles.flex, {marginLeft: theme.spacing.md}]}>
            <AppText variant="body" numberOfLines={1} style={styles.name}>{displayName}</AppText>
            <AppText variant="caption" color="textSecondary" style={{marginTop: theme.spacing.xs}}>{blocked ? 'Blocked' : id}</AppText>
          </View>
          {!blocked ? <AppText color="textMuted">›</AppText> : null}
        </Pressable>;
      }}
      ListEmptyComponent={storage.status === 'ready' && !conversations.error ? <AppText color="textMuted" style={{paddingVertical: theme.spacing.lg}}>{conversations.loading ? 'Loading contacts…' : 'People you message will appear here.'}</AppText> : undefined}
    />
  </Screen>;
}
const styles = StyleSheet.create({row: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth}, flex: {flex: 1}, name: {fontWeight: '600'}});
