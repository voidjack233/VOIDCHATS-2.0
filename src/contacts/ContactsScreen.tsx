import React, { useMemo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import type { Conversation } from '../conversations';
import { mockConversations } from '../conversations';
import { useTheme } from '../theme';
import { AppText, Avatar, Button, Divider, Screen, Surface, TextInput } from '../ui';
import { mockContacts } from './mockContacts';
import type { Contact } from './mockContacts';

export interface ContactsScreenProps {
  onOpenConversation: (conversation: Conversation) => void;
}

function conversationForContact(contact: Contact): Conversation {
  return mockConversations.find(conversation => conversation.id === contact.id) ?? {
    id: contact.id,
    name: contact.name,
    kind: 'direct',
    voidId: contact.voidId,
    preview: '',
    updatedAtLabel: '',
    unreadCount: 0,
  };
}

export function ContactsScreen({ onOpenConversation }: ContactsScreenProps) {
  const theme = useTheme();
  const [query, setQuery] = useState('');
  const visibleContacts = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return needle
      ? mockContacts.filter(contact =>
          `${contact.name} ${contact.voidId}`.toLocaleLowerCase().includes(needle),
        )
      : mockContacts;
  }, [query]);

  return (
    <Screen>
      <ScrollView
        contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}
        keyboardShouldPersistTaps="handled"
      >
        <AppText variant="title">Contacts</AppText>
        <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
          People you know, found by VOID-ID.
        </AppText>
        <View style={{ marginTop: theme.spacing.xl }}>
          <TextInput
            accessibilityLabel="Search saved contacts"
            placeholder="Search saved contacts"
            value={query}
            onChangeText={setQuery}
            autoCapitalize="none"
            returnKeyType="search"
          />
        </View>

        <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, { marginTop: theme.spacing.xxl, marginBottom: theme.spacing.sm }]}>
          SAVED CONTACTS
        </AppText>
        {visibleContacts.length > 0 ? visibleContacts.map((contact, index) => (
          <React.Fragment key={contact.id}>
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={`Open conversation with ${contact.name}`}
              onPress={() => onOpenConversation(conversationForContact(contact))}
              style={({ pressed }) => [styles.contactRow, {
                paddingVertical: theme.spacing.md,
                opacity: pressed ? 0.7 : 1,
              }]}
            >
              <Avatar name={contact.name} size={44} />
              <View style={[styles.contactContent, { marginLeft: theme.spacing.md }]}>
                <AppText variant="body" style={styles.contactName}>{contact.name}</AppText>
                <AppText variant="caption" color="textSecondary">{contact.voidId}</AppText>
              </View>
              <AppText variant="body" color="textMuted">›</AppText>
            </Pressable>
            {index < visibleContacts.length - 1 ? <Divider /> : null}
          </React.Fragment>
        )) : (
          <AppText variant="body" color="textSecondary" style={{ paddingVertical: theme.spacing.xl }}>
            No saved contacts match your search.
          </AppText>
        )}

        <Surface bordered padded style={{ marginTop: theme.spacing.xxl }}>
          <AppText variant="heading">Find someone by VOID-ID</AppText>
          <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
            VOID-ID lookup is coming soon.
          </AppText>
          <Button
            title="Search VOID-ID"
            variant="secondary"
            size="sm"
            disabled
            style={{ marginTop: theme.spacing.lg }}
          />
        </Surface>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  sectionLabel: { letterSpacing: 1 },
  contactRow: { flexDirection: 'row', alignItems: 'center' },
  contactContent: { flex: 1 },
  contactName: { fontWeight: '600' },
});
