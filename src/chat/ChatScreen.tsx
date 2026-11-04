import React, {useEffect, useMemo, useRef, useState} from 'react';
import {Alert, FlatList, KeyboardAvoidingView, Platform, StyleSheet, View} from 'react-native';
import type {Conversation} from '../conversations';
import {useMessaging} from '../messaging';
import {safeMessagingError} from '../messaging/errors';
import {useMessages, useStorage} from '../storage';
import type {MessageRecord} from '../storage/types';
import {useTheme} from '../theme';
import {AppText, Avatar, Button, IconButton, Screen, TextInput} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export interface ChatScreenProps {conversation: Conversation; onBack: () => void}

function dayLabel(timestamp: number): string {
  const date = new Date(timestamp);
  const today = new Date();
  if (date.toDateString() === today.toDateString()) { return 'Today'; }
  today.setDate(today.getDate() - 1);
  return date.toDateString() === today.toDateString() ? 'Yesterday' : date.toLocaleDateString(undefined, {month: 'short', day: 'numeric', year: 'numeric'});
}

function MessageRow({message, day, uncertain, paused, onRetry, retrying, retryDisabled}: {
  message: MessageRecord; day: boolean; uncertain: boolean; paused: boolean; onRetry: () => void; retrying: boolean; retryDisabled: boolean;
}) {
  const theme = useTheme();
  const outgoing = message.direction === 'outgoing';
  const status = uncertain ? 'Send status unknown' : message.deliveryState === 'pending' ? (paused ? 'Sending paused' : 'Sending…') : message.deliveryState === 'failed' ? 'Failed to send' : 'Sent to server';
  const reactions = new Map<string, number>();
  for (const reaction of message.reactions) { reactions.set(reaction.emoji, (reactions.get(reaction.emoji) ?? 0) + 1); }
  return <>
    {day ? <View style={styles.day}><AppText variant="caption" color="textMuted">{dayLabel(message.createdAt)}</AppText></View> : null}
    <View style={[styles.message, outgoing ? styles.outgoing : styles.incoming, {
      backgroundColor: outgoing ? theme.colors.accent : theme.colors.surface,
      paddingHorizontal: theme.spacing.lg, paddingVertical: theme.spacing.md,
      borderRadius: theme.radii.xl, borderBottomRightRadius: outgoing ? theme.radii.sm : theme.radii.xl,
      borderBottomLeftRadius: outgoing ? theme.radii.xl : theme.radii.sm,
    }]}>
      <AppText selectable style={[{color: outgoing ? theme.colors.accentText : theme.colors.textPrimary}, message.deletedAt !== null && styles.deletedText]}>
        {message.deletedAt !== null ? 'Message deleted' : message.body}
      </AppText>
      {reactions.size ? <AppText variant="caption" style={{marginTop: theme.spacing.sm, color: outgoing ? theme.colors.accentText : theme.colors.textSecondary}}>{Array.from(reactions, ([emoji, count]) => `${emoji} ${count}`).join('  ')}</AppText> : null}
      <AppText variant="caption" accessibilityLabel={new Date(message.createdAt).toLocaleString() + (outgoing ? ' · ' + status : '')}
        style={[styles.timestamp, outgoing && styles.outgoingTimestamp, {marginTop: theme.spacing.xs, color: outgoing ? theme.colors.accentText : theme.colors.textMuted}]}>
        {new Date(message.createdAt).toLocaleTimeString(undefined, {hour: 'numeric', minute: '2-digit'})}
        {message.editedAt !== null && message.deletedAt === null ? ' · Edited' : ''}
        {outgoing ? ' · ' + status : ''}
      </AppText>
    </View>
    {outgoing && message.deliveryState === 'failed' ? <View style={styles.failed}>
      {uncertain ? <AppText variant="caption" color="textMuted">Couldn’t confirm this send. Your message is saved.</AppText>
        : <Button title="Resend" variant="ghost" size="sm" loading={retrying} disabled={retryDisabled} onPress={onRetry} />}
    </View> : null}
  </>;
}

export function ChatScreen({conversation, onBack}: ChatScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const messages = useMessages(conversation.id);
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [retryingId, setRetryingId] = useState<string | null>(null);
  const [resuming, setResuming] = useState(false);
  const sendingRef = useRef(false);
  const retryingRef = useRef(false);
  const resumingRef = useRef(false);
  const mounted = useRef(true);
  const scope = `${storage.identity?.localDeviceId ?? ''}:${conversation.id}`;
  const currentScope = useRef(scope); currentScope.current = scope;
  const blocked = Boolean(conversation.voidId && messaging.blockedVoidIds.includes(conversation.voidId));
  const registered = storage.status === 'ready' && Boolean(storage.identity?.accountVoidId);
  const session = messaging.sessions.find(item => item.id === conversation.id);
  const diagnostic = session?.diagnosticCode ? safeMessagingError({code: session.diagnosticCode}) : null;
  const paused = diagnostic !== null && ['secure_state', 'identity_changed', 'storage', 'native_unavailable'].includes(diagnostic.code);
  const canCompose = registered && conversation.kind === 'direct' && !blocked;
  const list = useRef<FlatList<MessageRecord>>(null);
  const initiallyScrolled = useRef(false);
  const scrollAfterSend = useRef(false);
  const nearBottom = useRef(true);
  const readBatch = useRef('');
  const chronological = useMemo(() => [...messages.items].reverse(), [messages.items]);
  const name = conversation.name === `VOID-${conversation.voidId}` ? conversation.voidId : conversation.name;
  const active = () => mounted.current && currentScope.current === scope;

  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    initiallyScrolled.current = false; scrollAfterSend.current = false; nearBottom.current = true; readBatch.current = '';
    setDraft(''); setSendError(null); setSending(false); setRetryingId(null); setResuming(false);
    sendingRef.current = false; retryingRef.current = false; resumingRef.current = false;
  }, [scope]);

  async function send() {
    if (!draft.trim() || !canCompose || sendingRef.current) { return; }
    const body = draft;
    sendingRef.current = true; setSending(true); setSendError(null); scrollAfterSend.current = true;
    try {
      // Native storage saves the message immediately; network work continues separately.
      await messaging.sendMessage(conversation.id, body);
      if (active()) { setDraft(''); list.current?.scrollToEnd({animated: true}); }
    } catch (value) {
      if (active()) { scrollAfterSend.current = false; setSendError(value instanceof Error ? value.message : 'Could not save your message. Your text is still here.'); }
    } finally { if (active()) { sendingRef.current = false; setSending(false); } }
  }
  async function retry(messageId: string) {
    if (!canCompose || retryingRef.current) { return; }
    retryingRef.current = true; setRetryingId(messageId); setSendError(null);
    try { await messaging.retryMessage(messageId); }
    catch (value) { if (active()) { setSendError(value instanceof Error ? value.message : 'Could not retry this message.'); } }
    finally { if (active()) { retryingRef.current = false; setRetryingId(null); } }
  }
  async function resume() {
    if (!canCompose || resumingRef.current) { return; }
    resumingRef.current = true; setResuming(true); setSendError(null);
    try { await messaging.refresh(); }
    catch (value) { if (active()) { setSendError(safeMessagingError(value).message); } }
    finally { if (active()) { resumingRef.current = false; setResuming(false); } }
  }
  function options() {
    if (!conversation.voidId || blocked) { return; }
    const peer = conversation.voidId;
    Alert.alert(name ?? peer, undefined, [{text: 'Cancel', style: 'cancel'}, {text: 'Block person', style: 'destructive', onPress: () => {
      messaging.blockPeer(peer).catch(value => { if (active()) { setSendError(value instanceof Error ? value.message : 'Could not block this person.'); } });
    }}]);
  }

  useEffect(() => {
    if (storage.status !== 'ready' || messages.loading || messages.error) { return; }
    const unread = messages.items.filter(message => message.direction === 'incoming' && message.readAt === null && message.deletedAt === null);
    const batch = unread.map(message => message.id).sort().join('\n');
    if (!batch || batch === readBatch.current) { return; }
    readBatch.current = batch;
    storage.repositories.conversations.markRead(conversation.id, {createdAt: unread[0].createdAt, id: unread[0].id}).catch(() => { readBatch.current = ''; });
  }, [conversation.id, messages.items, messages.loading, messages.error, storage.status, storage.repositories]);

  return <Screen>
    <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
      <View style={[styles.header, {paddingHorizontal: theme.spacing.sm, paddingVertical: theme.spacing.sm, borderBottomColor: theme.colors.border}]}>
        <IconButton accessibilityLabel="Back to messages" onPress={onBack} icon={<AppText variant="title">‹</AppText>} />
        <Avatar name={name ?? conversation.voidId ?? 'VOID'} size={36} />
        <View style={[styles.flex, {marginLeft: theme.spacing.md}]}>
          <AppText variant="body" style={styles.name} numberOfLines={1}>{name}</AppText>
          {name !== conversation.voidId && conversation.voidId ? <AppText variant="caption" color="textSecondary" numberOfLines={1}>{conversation.voidId}</AppText> : null}
        </View>
        <IconButton accessibilityLabel="Chat options" disabled={blocked || !conversation.voidId} onPress={options} icon={<AppText variant="heading" color="textSecondary">⋮</AppText>} />
      </View>
      <FlatList
        ref={list}
        data={chronological}
        keyExtractor={item => item.id}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        renderItem={({item, index}) => <MessageRow message={item}
          day={index === 0 || new Date(chronological[index - 1].createdAt).toDateString() !== new Date(item.createdAt).toDateString()}
          uncertain={messaging.outbox.some(operation => operation.id === item.id && ['duplicate_event', 'send_unknown'].includes(operation.errorCode ?? ''))}
          paused={paused}
          onRetry={() => { retry(item.id).catch(() => {}); }} retrying={retryingId === item.id} retryDisabled={!canCompose || paused || retryingId !== null} />}
        maintainVisibleContentPosition={{minIndexForVisible: 0}}
        contentContainerStyle={[styles.messages, {paddingHorizontal: theme.spacing.lg, paddingBottom: theme.spacing.lg}]}
        scrollEventThrottle={100}
        onScroll={({nativeEvent}) => { nearBottom.current = nativeEvent.contentSize.height - nativeEvent.layoutMeasurement.height - nativeEvent.contentOffset.y < 80; }}
        onContentSizeChange={() => {
          if ((!initiallyScrolled.current || scrollAfterSend.current || nearBottom.current) && chronological.length > 0) {
            initiallyScrolled.current = true; scrollAfterSend.current = false; list.current?.scrollToEnd({animated: false});
          }
        }}
        ListHeaderComponent={<>
          <StorageStatus />
          {session?.isRequest ? <AppText variant="caption" color="textMuted" style={[styles.centeredText, {marginVertical: theme.spacing.lg}]}>Message request · Reply to move this chat to Messages.</AppText> : null}
          {messages.hasMore && registered ? <Button title="Earlier messages" variant="ghost" loading={messages.loading} onPress={() => { messages.loadMore(); }} style={[styles.centeredSelf, {marginTop: theme.spacing.sm}]} /> : null}
          {messages.error && registered ? <View style={{paddingVertical: theme.spacing.lg}}><AppText color="danger">Could not load messages.</AppText><Button title="Try again" variant="ghost" onPress={messages.refresh} /></View> : null}
        </>}
        ListEmptyComponent={storage.status === 'ready' && !messages.error ? <View style={styles.empty}>
          <AppText color="textMuted">{messages.loading ? 'Loading messages…' : 'Say hello.'}</AppText>
        </View> : undefined}
      />
      <View style={{paddingHorizontal: theme.spacing.md, paddingTop: theme.spacing.sm, paddingBottom: theme.spacing.md}}>
        {sendError ? <AppText color="danger" variant="caption" accessibilityLiveRegion="polite" style={{marginBottom: theme.spacing.sm}}>{sendError}</AppText> : null}
        {diagnostic && paused && !blocked && registered ? <View style={[styles.paused, {paddingHorizontal: theme.spacing.md, paddingVertical: theme.spacing.sm, marginBottom: theme.spacing.sm, backgroundColor: theme.colors.surface, borderRadius: theme.radii.md}]}>
          <View style={styles.flex}>
            <AppText variant="caption" color="warning" accessibilityLiveRegion="polite">Your messages are saved. Sending is paused.</AppText>
            {diagnostic.code === 'identity_changed' ? <AppText variant="caption" color="textSecondary" style={{marginTop: theme.spacing.xs}}>{diagnostic.message}</AppText> : null}
            {session?.diagnosticCode ? <AppText variant="caption" color="textMuted" selectable style={{marginTop: theme.spacing.xs}}>Error code: {session.diagnosticCode}</AppText> : null}
          </View>
          <Button title="Reconnect" variant="ghost" size="sm" loading={resuming} onPress={() => { resume().catch(() => {}); }} style={{marginLeft: theme.spacing.sm}} />
        </View> : null}
        {blocked || !registered || conversation.kind !== 'direct' || messaging.status === 'offline' ? <AppText variant="caption" color="textMuted" accessibilityLiveRegion="polite" style={[styles.centeredText, {marginBottom: theme.spacing.sm}]}>
          {blocked ? 'You blocked this person.' : !registered ? 'Create an account to send messages.' : conversation.kind !== 'direct' ? 'Group messaging is not available yet.' : 'You’re offline. Failed messages stay saved until you tap Resend.'}
        </AppText> : null}
        <View style={styles.composerRow}>
          <TextInput accessibilityLabel="Message" placeholder={blocked ? 'Person blocked' : 'Message'} value={draft} onChangeText={setDraft}
            editable={canCompose && !sending} multiline maxLength={16000} containerStyle={styles.flex}
            style={[styles.input, {paddingHorizontal: theme.spacing.lg, paddingTop: theme.spacing.md, paddingBottom: theme.spacing.md}]} />
          <Button title="↑" accessibilityLabel="Send message" loading={sending} disabled={!canCompose || !draft.trim()} onPress={() => { send().catch(() => {}); }}
            style={[styles.sendButton, {marginLeft: theme.spacing.sm}]} textStyle={styles.sendText} />
        </View>
      </View>
    </KeyboardAvoidingView>
  </Screen>;
}
const styles = StyleSheet.create({
  flex: {flex: 1}, header: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth},
  messages: {flexGrow: 1}, empty: {flex: 1, justifyContent: 'center', alignItems: 'center', minHeight: 160},
  day: {alignItems: 'center', paddingTop: 22, paddingBottom: 16},
  message: {maxWidth: '86%', marginBottom: 7}, outgoing: {alignSelf: 'flex-end'}, incoming: {alignSelf: 'flex-start'},
  failed: {alignSelf: 'flex-end', maxWidth: '86%', marginBottom: 8}, composerRow: {flexDirection: 'row', alignItems: 'flex-end'},
  paused: {flexDirection: 'row', alignItems: 'center'},
  timestamp: {fontSize: 11, textAlign: 'right'}, outgoingTimestamp: {opacity: 0.7}, deletedText: {opacity: 0.6}, name: {fontWeight: '600'},
  centeredText: {textAlign: 'center'}, centeredSelf: {alignSelf: 'center'}, input: {maxHeight: 140, minHeight: 46, borderRadius: 24},
  sendButton: {width: 46, minHeight: 46, borderRadius: 23, paddingHorizontal: 0}, sendText: {fontSize: 24},
});
