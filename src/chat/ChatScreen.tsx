import React from 'react';
import { ScrollView, StyleSheet, View } from 'react-native';
import type { Conversation } from '../conversations';
import { useTheme } from '../theme';
import { AppText, Avatar, Button, IconButton, Screen, TextInput } from '../ui';
import { mockMessages } from './mockMessages';

export interface ChatScreenProps {
  conversation: Conversation;
  onBack: () => void;
}

export function ChatScreen({ conversation, onBack }: ChatScreenProps) {
  const theme = useTheme();
  const messages = mockMessages.filter(message => message.conversationId === conversation.id);

  return (
    <Screen>
      <View
        style={[styles.header, {
          paddingHorizontal: theme.spacing.md,
          paddingVertical: theme.spacing.sm,
          borderBottomColor: theme.colors.border,
        }]}
      >
        <IconButton
          accessibilityLabel="Back to conversations"
          onPress={onBack}
          icon={<AppText variant="heading">‹</AppText>}
          style={{ marginRight: theme.spacing.xs }}
        />
        <Avatar name={conversation.name} size={40} />
        <View style={[styles.flex, { marginLeft: theme.spacing.md }]}>
          <AppText variant="label" numberOfLines={1}>{conversation.name}</AppText>
          <AppText variant="caption" color="textSecondary" numberOfLines={1}>
            {conversation.kind === 'group' ? 'Group conversation' : conversation.voidId ?? 'Direct conversation'}
          </AppText>
        </View>
      </View>

      <ScrollView
        contentContainerStyle={[styles.messages, messages.length > 0 ? styles.messagesEnd : styles.messagesCenter, {
          paddingHorizontal: theme.spacing.lg,
          paddingVertical: theme.spacing.xl,
        }]}
      >
        {messages.length > 0 ? messages.map(message => {
          const outgoing = message.direction === 'outgoing';
          return (
            <View
              key={message.id}
              style={[styles.bubbleWrapper, outgoing ? styles.alignOutgoing : styles.alignIncoming, {
                marginBottom: theme.spacing.md,
              }]}
            >
              <View
                style={[styles.bubble, {
                  backgroundColor: outgoing ? theme.colors.surfaceElevated : theme.colors.surface,
                  borderColor: outgoing ? theme.colors.accent : theme.colors.border,
                  borderRadius: theme.radii.lg,
                  paddingHorizontal: theme.spacing.md,
                  paddingVertical: theme.spacing.sm,
                }]}
              >
                <AppText variant="body">{message.body}</AppText>
              </View>
              <AppText
                variant="caption"
                color="textMuted"
                style={[outgoing ? styles.alignOutgoing : styles.alignIncoming, { marginTop: theme.spacing.xs }]}
              >
                {message.timeLabel}
              </AppText>
            </View>
          );
        }) : (
          <View style={styles.empty}>
            <AppText variant="heading">No messages yet</AppText>
            <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
              Your conversation will appear here.
            </AppText>
          </View>
        )}
      </ScrollView>

      <View
        style={[styles.composer, {
          paddingHorizontal: theme.spacing.lg,
          paddingTop: theme.spacing.md,
          paddingBottom: theme.spacing.lg,
          borderTopColor: theme.colors.border,
        }]}
      >
        <View style={styles.composerRow}>
          <TextInput
            accessibilityLabel="Message composer, coming soon"
            placeholder="Message"
            editable={false}
            containerStyle={styles.flex}
            style={styles.disabledInput}
          />
          <Button title="Send" disabled size="sm" style={{ marginLeft: theme.spacing.sm }} />
        </View>
        <AppText variant="caption" color="textMuted" style={{ marginTop: theme.spacing.sm }}>
          Messaging is coming soon.
        </AppText>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  header: { flexDirection: 'row', alignItems: 'center', borderBottomWidth: 1 },
  flex: { flex: 1 },
  messages: { flexGrow: 1 },
  messagesEnd: { justifyContent: 'flex-end' },
  messagesCenter: { justifyContent: 'center' },
  bubbleWrapper: { maxWidth: '82%' },
  alignOutgoing: { alignSelf: 'flex-end' },
  alignIncoming: { alignSelf: 'flex-start' },
  bubble: { borderWidth: 1 },
  empty: { alignItems: 'center' },
  composer: { borderTopWidth: 1 },
  composerRow: { flexDirection: 'row', alignItems: 'center' },
  disabledInput: { opacity: 0.6 },
});
