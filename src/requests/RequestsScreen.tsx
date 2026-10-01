import React from 'react';
import { Pressable, ScrollView, StyleSheet, View } from 'react-native';
import type { Conversation } from '../conversations';
import { useTheme } from '../theme';
import { AppText, Avatar, Divider, Screen } from '../ui';
import { mockRequests } from './mockRequests';
import type { MessageRequest } from './mockRequests';

export interface RequestsScreenProps {
  onOpenConversation?: (conversation: Conversation) => void;
}

function conversationForRequest(request: MessageRequest): Conversation {
  return {
    id: `request-${request.id}`,
    name: request.name,
    kind: 'direct',
    voidId: request.voidId,
    preview: request.preview,
    updatedAtLabel: request.receivedAtLabel,
    unreadCount: 0,
  };
}

export function RequestsScreen({ onOpenConversation }: RequestsScreenProps) {
  const theme = useTheme();

  return (
    <Screen>
      <ScrollView contentContainerStyle={{ paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xxl }}>
        <AppText variant="title">Message requests</AppText>
        <AppText variant="body" color="textSecondary" style={{ marginTop: theme.spacing.sm }}>
          Messages from people outside your contacts appear here.
        </AppText>
        <View style={{ marginTop: theme.spacing.xxl }}>
          {mockRequests.map((request, index) => (
            <React.Fragment key={request.id}>
              <Pressable
                accessibilityRole="button"
                accessibilityLabel={`Preview request from ${request.name}`}
                accessibilityState={{ disabled: !onOpenConversation }}
                disabled={!onOpenConversation}
                onPress={() => onOpenConversation?.(conversationForRequest(request))}
                style={({ pressed }) => [styles.requestRow, {
                  paddingVertical: theme.spacing.lg,
                  opacity: pressed ? 0.7 : 1,
                }]}
              >
                <Avatar name={request.name} size={48} />
                <View style={[styles.requestContent, { marginLeft: theme.spacing.md }]}>
                  <View style={styles.headingRow}>
                    <AppText variant="body" numberOfLines={1} style={styles.name}>
                      {request.name}
                    </AppText>
                    <AppText variant="caption" color="textMuted" style={{ marginLeft: theme.spacing.sm }}>
                      {request.receivedAtLabel}
                    </AppText>
                  </View>
                  <AppText variant="caption" color="accent" style={{ marginTop: theme.spacing.xs }}>
                    {request.voidId}
                  </AppText>
                  <AppText variant="body" color="textSecondary" numberOfLines={2} style={{ marginTop: theme.spacing.sm }}>
                    {request.preview}
                  </AppText>
                </View>
              </Pressable>
              {index < mockRequests.length - 1 ? <Divider /> : null}
            </React.Fragment>
          ))}
        </View>
        <View style={[styles.footer, { borderTopColor: theme.colors.border, marginTop: theme.spacing.xl, paddingTop: theme.spacing.lg }]}>
          <AppText variant="caption" color="textMuted">
            Accepting requests is coming soon.
          </AppText>
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  requestRow: { flexDirection: 'row', alignItems: 'flex-start' },
  requestContent: { flex: 1 },
  headingRow: { flexDirection: 'row', alignItems: 'baseline' },
  name: { flex: 1, fontWeight: '600' },
  footer: { borderTopWidth: 1 },
});
