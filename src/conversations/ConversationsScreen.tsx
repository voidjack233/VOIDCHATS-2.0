import React from 'react';
import { FlatList, Pressable, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Button, Divider, Screen } from '../ui';
import {StorageStatus} from '../ui/StorageStatus';
import {useConversations, useStorage} from '../storage';
import {ConversationRow} from './ConversationRow';
import {conversationFromRecord, type Conversation} from './types';

export interface ConversationsScreenProps {
  onOpenRequests: () => void;
  onOpenContacts: () => void;
  onOpenConversation: (conversation: Conversation) => void;
}

export function ConversationsScreen({
  onOpenRequests,
  onOpenContacts,
  onOpenConversation,
}: ConversationsScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const {items, loading, error, refresh, loadMore, hasMore} = useConversations();

  return (
    <Screen>
      <FlatList
        data={items}
        keyExtractor={item => item.id}
        refreshing={loading && storage.status === 'ready'}
        onRefresh={() => { refresh(); }}
        contentContainerStyle={{paddingBottom: theme.spacing.xxl}}
        ItemSeparatorComponent={Divider}
        ListFooterComponent={storage.status === 'ready' && items.length > 0 ? (
          <View style={{padding: theme.spacing.xl}}>
            {error ? <AppText variant="body" color="danger" style={{marginBottom: theme.spacing.md}}>Could not load more conversations.</AppText> : null}
            {hasMore ? <Button title="Load more conversations" variant="secondary" loading={loading} onPress={loadMore} /> : null}
            {error && !hasMore ? <Button title="Try again" variant="secondary" onPress={refresh} /> : null}
          </View>
        ) : null}
        renderItem={({item}) => {
          const conversation = conversationFromRecord(item);
          return <View style={{paddingHorizontal: theme.spacing.xl}}><ConversationRow conversation={conversation} onPress={() => onOpenConversation(conversation)} /></View>;
        }}
        ListHeaderComponent={<>
      <View style={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl }}>
        <View style={styles.headerRow}>
          <View>
            <AppText variant="caption" color="accent" style={styles.brand}>
              VOID
            </AppText>
            <AppText variant="title" style={{ marginTop: theme.spacing.xs }}>
              Conversations
            </AppText>
          </View>
          <Pressable
            accessibilityRole="button"
            onPress={onOpenContacts}
            style={({ pressed }) => ({ padding: theme.spacing.sm, opacity: pressed ? 0.65 : 1 })}
          >
            <AppText variant="label" color="accent">Find people</AppText>
          </Pressable>
        </View>
        <Pressable
          accessibilityRole="button"
          onPress={onOpenRequests}
          style={({ pressed }) => [styles.shortcut, {
            marginTop: theme.spacing.xl,
            paddingVertical: theme.spacing.lg,
            opacity: pressed ? 0.65 : 1,
          }]}
        >
          <AppText variant="label" style={styles.flex}>Message requests</AppText>
          <AppText variant="body" color="textMuted">›</AppText>
        </Pressable>
      </View>
      <Divider />
      <View style={{paddingHorizontal: theme.spacing.xl}}><StorageStatus /></View>
      </>}
        ListEmptyComponent={storage.status === 'ready' ? (
          error ? <View style={{padding: theme.spacing.xl}}>
            <AppText variant="body" color="danger">Could not load conversations.</AppText>
            <Button title="Try again" variant="secondary" onPress={() => { refresh(); }} style={{marginTop: theme.spacing.md}} />
          </View> : loading ? <AppText variant="body" color="textSecondary" style={{padding: theme.spacing.xl}}>Loading conversations…</AppText> :
        <View style={[styles.emptyState, { paddingTop: theme.spacing.xxxl }]}>
          <AppText variant="heading">No conversations yet</AppText>
          <AppText variant="body" color="textSecondary" style={[styles.emptyDescription, { marginTop: theme.spacing.sm }]}>
            No conversations are stored on this device yet.
          </AppText>
        </View>
        ) : null}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  headerRow: { flexDirection: 'row', alignItems: 'flex-end', justifyContent: 'space-between' },
  brand: { letterSpacing: 2, fontWeight: '700' },
  shortcut: { flexDirection: 'row', alignItems: 'center' },
  flex: { flex: 1 },
  emptyState: { alignItems: 'center' },
  emptyDescription: { textAlign: 'center' },
});
