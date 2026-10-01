import React, { useMemo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { useTheme } from '../theme';
import { AppText, Divider, Screen, TextInput } from '../ui';
import { ConversationRow } from './ConversationRow';
import { mockConversations } from './mockConversations';
import type { Conversation } from './types';

export interface ConversationsScreenProps {
  onOpenConversation: (conversation: Conversation) => void;
  onOpenRequests: () => void;
  onOpenContacts: () => void;
}

export function ConversationsScreen({
  onOpenConversation,
  onOpenRequests,
  onOpenContacts,
}: ConversationsScreenProps) {
  const theme = useTheme();
  const [query, setQuery] = useState('');
  const visibleConversations = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return needle
      ? mockConversations.filter(conversation =>
          `${conversation.name} ${conversation.preview}`.toLocaleLowerCase().includes(needle),
        )
      : mockConversations;
  }, [query]);

  return (
    <Screen>
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
        <View style={{ marginTop: theme.spacing.xl }}>
          <TextInput
            accessibilityLabel="Search conversations"
            placeholder="Search conversations"
            value={query}
            onChangeText={setQuery}
            autoCapitalize="none"
            returnKeyType="search"
          />
        </View>
        <Pressable
          accessibilityRole="button"
          onPress={onOpenRequests}
          style={({ pressed }) => [styles.shortcut, {
            paddingVertical: theme.spacing.lg,
            opacity: pressed ? 0.65 : 1,
          }]}
        >
          <AppText variant="label" style={styles.flex}>Message requests</AppText>
          <AppText variant="body" color="textMuted">›</AppText>
        </Pressable>
      </View>
      <Divider />
      <ScrollView
        contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}
        keyboardShouldPersistTaps="handled"
      >
        {visibleConversations.length > 0 ? (
          visibleConversations.map((conversation, index) => (
            <React.Fragment key={conversation.id}>
              <ConversationRow
                conversation={conversation}
                onPress={() => onOpenConversation(conversation)}
              />
              {index < visibleConversations.length - 1 ? <Divider /> : null}
            </React.Fragment>
          ))
        ) : (
          <View style={[styles.emptyState, { paddingTop: theme.spacing.xxxl }]}>
            <AppText variant="heading">No conversations found</AppText>
            <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
              Try a different name or phrase.
            </AppText>
          </View>
        )}
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  headerRow: { flexDirection: 'row', alignItems: 'flex-end', justifyContent: 'space-between' },
  brand: { letterSpacing: 2, fontWeight: '700' },
  shortcut: { flexDirection: 'row', alignItems: 'center' },
  flex: { flex: 1 },
  emptyState: { alignItems: 'center' },
});
