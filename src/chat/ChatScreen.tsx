import React, {useEffect, useMemo, useRef, useState} from 'react';
import {Alert, FlatList, KeyboardAvoidingView, Platform, Pressable, StyleSheet, View} from 'react-native';
import {useSafeAreaInsets} from 'react-native-safe-area-context';
import type {Conversation} from '../conversations';
import {useMessaging} from '../messaging';
import {safeMessagingError} from '../messaging/errors';
import {useMessages, useStorage} from '../storage';
import type {MessageRecord} from '../storage/types';
import {useTheme} from '../theme';
import {AppText, Avatar, Button, IconButton, Screen, TextInput} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';
import {getMessageLayout} from './messageLayout';

export interface ChatScreenProps {conversation: Conversation; onBack: () => void}

function ChatKeyboardLayout({children}: {children: React.ReactNode}) {
  // Android reports the keyboard in screen coordinates, above the chat's local origin.
  const {top} = useSafeAreaInsets();
  return (
    <KeyboardAvoidingView
      style={styles.flex}
      behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
      keyboardVerticalOffset={Platform.OS === 'android' ? top : 0}>
      {children}
    </KeyboardAvoidingView>
  );
}

function dayLabel(timestamp: number): string {
  const date = new Date(timestamp);
  const today = new Date();
  if (date.toDateString() === today.toDateString()) { return 'Today'; }
  today.setDate(today.getDate() - 1);
  return date.toDateString() === today.toDateString() ? 'Yesterday' : date.toLocaleDateString(undefined, {month: 'short', day: 'numeric', year: 'numeric'});
}

const deliveryIndicators = {
  pending: {label: 'Waiting to send'},
  sent: {label: 'Sent'},
  delivered: {label: 'Delivered'},
  failed: {label: 'Failed to send'},
  unknown: {label: 'Send status unknown'},
} as const;

function DeliveryIndicator({status, label}: {status: keyof typeof deliveryIndicators; label: string}) {
  const {colors} = useTheme();
  const stroke = colors.accentText;
  if (status === 'delivered') {
    return <View accessible accessibilityRole="image" accessibilityLabel={label} style={styles.mailIcon}>
      <View style={[styles.mailOutline, {borderColor: stroke}]} />
      <View style={[styles.mailFlapLeft, {backgroundColor: stroke}]} />
      <View style={[styles.mailFlapRight, {backgroundColor: stroke}]} />
      <View style={[styles.mailCheckBackground, {backgroundColor: colors.accent}]}>
        <View style={[styles.mailCheck, {borderColor: stroke}]} />
      </View>
    </View>;
  }
  return (
    <View
      accessible
      accessibilityRole="image"
      accessibilityLabel={label}
      style={[styles.deliveryIcon, {borderColor: stroke}]}>
      {status === 'failed' || status === 'unknown' ? (
        <View style={styles.exclamation}>
          <View style={[styles.exclamationBar, {backgroundColor: stroke}]} />
          <View style={[styles.exclamationDot, {backgroundColor: stroke}]} />
        </View>
      ) : (
        <View style={[styles.deliveryMark, status === 'pending' ? styles.clockHands : styles.checkmark, {borderColor: stroke}]} />
      )}
    </View>
  );
}

function MessageRow({message, day, startsGroup, endsGroup, uncertain, paused, onRetry, retrying, retryDisabled}: {
  message: MessageRecord; day: boolean; startsGroup: boolean; endsGroup: boolean;
  uncertain: boolean; paused: boolean;
  onRetry: () => void; retrying: boolean; retryDisabled: boolean;
}) {
  const theme = useTheme();
  const outgoing = message.direction === 'outgoing';
  const {deliveryState} = message;
  const delivered = outgoing && typeof message.verifiedDeliveredAt === 'number';
  const [showPending, setShowPending] = useState(false);
  useEffect(() => {
    setShowPending(false);
    if (outgoing && deliveryState === 'pending' && !delivered) {
      const timer = setTimeout(() => setShowPending(true), 500);
      return () => clearTimeout(timer);
    }
  }, [message.id, outgoing, deliveryState, delivered]);
  const failed = outgoing && !delivered && deliveryState === 'failed';
  const status = !outgoing ? null
    : delivered ? 'delivered'
    : uncertain ? 'unknown'
    : failed ? 'failed'
    : deliveryState === 'pending' || deliveryState === 'sent' ? deliveryState
    : null;
  const visibleStatus = status === 'pending' && !showPending ? null : status;
  const statusLabel = status === 'pending' && paused ? 'Sending paused' : status ? deliveryIndicators[status].label : '';
  const timestamp = new Date(message.createdAt);
  const metadataColor = outgoing ? theme.colors.accentText : theme.colors.textSecondary;
  const joinedCorners = {
    borderTopLeftRadius: !outgoing && !startsGroup ? theme.radii.sm : theme.radii.xl,
    borderTopRightRadius: outgoing && !startsGroup ? theme.radii.sm : theme.radii.xl,
    borderBottomLeftRadius: !outgoing && !endsGroup ? theme.radii.sm : theme.radii.xl,
    borderBottomRightRadius: outgoing && !endsGroup ? theme.radii.sm : theme.radii.xl,
  };
  const reactions = new Map<string, number>();
  for (const reaction of message.reactions) { reactions.set(reaction.emoji, (reactions.get(reaction.emoji) ?? 0) + 1); }
  function showMessageOptions() {
    if (uncertain) {
      Alert.alert('Send status unknown', 'Couldn’t confirm this send. Your message is saved.');
    } else if (retryDisabled || retrying) {
      Alert.alert('Resend unavailable', retrying ? 'Resending this message…' : 'Resend is currently unavailable.');
    } else {
      Alert.alert('Message options', undefined, [{text: 'Cancel', style: 'cancel'}, {text: 'Resend', onPress: onRetry}]);
    }
  }
  // The inverted list reverses direct cell children, so each row needs one root.
  return <View style={{paddingTop: startsGroup ? theme.spacing.md : theme.spacing.xs}}>
    {day ? <View style={[styles.day, {
      backgroundColor: theme.colors.surface, borderRadius: theme.radii.full, marginBottom: theme.spacing.md,
      paddingHorizontal: theme.spacing.md, paddingVertical: theme.spacing.xs,
    }]}>
      <AppText variant="caption" color="textSecondary">{dayLabel(message.createdAt)}</AppText>
    </View> : null}
    <View style={[styles.messageGroup, outgoing ? styles.outgoing : styles.incoming]}>
      <View style={styles.bubbleRow}>
      {failed ? <Pressable accessibilityRole="button" accessibilityLabel={uncertain ? 'Send status unknown' : 'Message failed'} accessibilityHint={uncertain ? 'Shows send status information.' : 'Shows how to resend this message.'}
        onPress={() => uncertain ? showMessageOptions() : Alert.alert('Message failed', 'Hold the message for options.')}
        onAccessibilityAction={({nativeEvent}) => { if (nativeEvent.actionName === 'longpress') { showMessageOptions(); } }}
        accessibilityActions={[{name: 'longpress', label: 'Message options'}]}
        hitSlop={12} style={[styles.failureIndicator, {borderColor: theme.colors.warning, marginRight: theme.spacing.sm}]}>
        <AppText style={[styles.failureMark, {color: theme.colors.warning}]}>!</AppText>
      </Pressable> : null}
      <Pressable onLongPress={failed ? showMessageOptions : undefined} accessibilityActions={failed ? [{name: 'longpress', label: 'Message options'}] : undefined}
        onAccessibilityAction={failed ? ({nativeEvent}) => { if (nativeEvent.actionName === 'longpress') { showMessageOptions(); } } : undefined}
        style={[styles.bubble, joinedCorners, {
        backgroundColor: outgoing ? theme.colors.accent : theme.colors.surface,
        paddingHorizontal: theme.spacing.md, paddingVertical: theme.spacing.sm,
      }]}>
        <AppText selectable={!failed} style={[{color: outgoing ? theme.colors.accentText : theme.colors.textPrimary}, message.deletedAt !== null && styles.deletedText]}>
          {message.deletedAt !== null ? 'Message deleted' : message.body}
        </AppText>
        {reactions.size ? <AppText variant="caption" style={{marginTop: theme.spacing.sm, color: outgoing ? theme.colors.accentText : theme.colors.textSecondary}}>{Array.from(reactions, ([emoji, count]) => `${emoji} ${count}`).join('  ')}</AppText> : null}
        <View style={styles.metadata}>
          {message.editedAt !== null && message.deletedAt === null ? <AppText variant="caption" style={{color: metadataColor}}>Edited</AppText> : null}
          <AppText variant="caption" accessibilityLabel={timestamp.toLocaleString() + (!visibleStatus && statusLabel ? `, ${statusLabel}` : '')}
            style={[styles.timestamp, {color: metadataColor}]}>
            {timestamp.toLocaleTimeString(undefined, {hour: 'numeric', minute: '2-digit'})}
          </AppText>
          {visibleStatus ? <DeliveryIndicator status={visibleStatus} label={statusLabel} /> : null}
        </View>
      </Pressable>
      </View>
    </View>
  </View>;
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
  const readBatch = useRef('');
  const messageLayout = useMemo(() => getMessageLayout(messages.items, messages.hasMore), [messages.items, messages.hasMore]);
  const name = conversation.name === `VOID-${conversation.voidId}` ? conversation.voidId : conversation.name;
  const active = () => mounted.current && currentScope.current === scope;

  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    readBatch.current = '';
    setDraft(''); setSendError(null); setSending(false); setRetryingId(null); setResuming(false);
    sendingRef.current = false; retryingRef.current = false; resumingRef.current = false;
  }, [scope]);

  async function send() {
    if (!draft.trim() || !canCompose || sendingRef.current) { return; }
    const body = draft;
    sendingRef.current = true; setSending(true); setSendError(null);
    try {
      // Native storage saves the message immediately; network work continues separately.
      await messaging.sendMessage(conversation.id, body);
      if (active()) { setDraft(''); }
    } catch (value) {
      if (active()) { setSendError(value instanceof Error ? value.message : 'Could not save your message. Your text is still here.'); }
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
    try { await messaging.reconnect(); }
    catch (value) { if (active()) { setSendError(safeMessagingError(value).message); } }
    finally { if (active()) { resumingRef.current = false; setResuming(false); } }
  }
  async function logSendDiagnostics() {
    try {
      const count = await messaging.logSendDiagnostics(conversation.id);
      if (active()) { Alert.alert('Send diagnostics', count === 0 ? 'No saved outgoing send records were found for this chat.' : `Requested local Android diagnostic logging for ${count} saved outgoing messages.`); }
    } catch (value) {
      if (active()) { setSendError(safeMessagingError(value).message); }
    }
  }
  function options() {
    if (!conversation.voidId || blocked) { return; }
    const peer = conversation.voidId;
    Alert.alert(name ?? peer, undefined, [{text: 'Cancel', style: 'cancel'}, {text: 'Log send diagnostics', onPress: () => { logSendDiagnostics().catch(() => {}); }}, {text: 'Block person', style: 'destructive', onPress: () => {
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
    <ChatKeyboardLayout>
      <View style={[styles.header, {paddingHorizontal: theme.spacing.sm, paddingVertical: theme.spacing.sm, borderBottomColor: theme.colors.border}]}>
        <IconButton accessibilityLabel="Back to messages" onPress={onBack} icon={<AppText variant="title">‹</AppText>} />
        <Avatar name={name ?? conversation.voidId ?? 'VOID'} size={36} />
        <View style={[styles.flex, {marginLeft: theme.spacing.md}]}>
          <AppText variant="body" style={styles.name} numberOfLines={1}>{name}</AppText>
          {name !== conversation.voidId && conversation.voidId ? <AppText variant="caption" color="textSecondary" numberOfLines={1}>{conversation.voidId}</AppText> : null}
        </View>
        <IconButton accessibilityLabel="Chat options" disabled={blocked || !conversation.voidId} onPress={options} icon={<AppText variant="heading" color="textSecondary">⋮</AppText>} />
      </View>
      {/* History is newest first, so the inverted list starts at the composer end. */}
      <FlatList
        inverted
        data={messages.items}
        keyExtractor={item => item.id}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        renderItem={({item, index}) => {
          const layout = messageLayout.get(item.id);
          const newer = messages.items[index - 1];
          return <MessageRow message={item}
            day={layout?.showDateSeparator ?? false}
            startsGroup={layout?.startsGroup ?? true}
            endsGroup={newer ? messageLayout.get(newer.id)?.startsGroup ?? true : true}
            uncertain={messaging.outbox.some(operation => operation.id === item.id && ['duplicate_event', 'send_unknown'].includes(operation.errorCode ?? ''))}
            paused={paused}
            onRetry={() => { retry(item.id).catch(() => {}); }} retrying={retryingId === item.id} retryDisabled={!canCompose || paused || retryingId !== null} />;
        }}
        maintainVisibleContentPosition={{minIndexForVisible: 0, autoscrollToTopThreshold: 80}}
        contentContainerStyle={[styles.messages, {paddingHorizontal: theme.spacing.lg, paddingTop: theme.spacing.lg}]}
        ListFooterComponent={<>
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
        {!blocked && registered && conversation.kind === 'direct' && messaging.status === 'offline' ? <View pointerEvents="none" style={[styles.offlineNotice, {left: theme.spacing.lg, right: theme.spacing.lg, bottom: '100%', backgroundColor: theme.colors.surfaceElevated, borderRadius: theme.radii.md, padding: theme.spacing.sm}]}>
          <AppText variant="caption" color="textSecondary" accessibilityLiveRegion="polite" style={styles.centeredText}>You’re offline. Failed messages stay saved until you tap Resend.</AppText>
        </View> : null}
        {sendError ? <AppText color="danger" variant="caption" accessibilityLiveRegion="polite" style={{marginBottom: theme.spacing.sm}}>{sendError}</AppText> : null}
        {diagnostic && paused && !blocked && registered ? <View style={[styles.paused, {paddingHorizontal: theme.spacing.md, paddingVertical: theme.spacing.sm, marginBottom: theme.spacing.sm, backgroundColor: theme.colors.surface, borderRadius: theme.radii.md}]}>
          <View style={styles.flex}>
            <AppText variant="caption" color="warning" accessibilityLiveRegion="polite">Your messages are saved. Sending is paused.</AppText>
            {diagnostic.code === 'identity_changed' ? <AppText variant="caption" color="textSecondary" style={{marginTop: theme.spacing.xs}}>{diagnostic.message}</AppText> : null}
            {session?.diagnosticCode ? <AppText variant="caption" color="textMuted" selectable style={{marginTop: theme.spacing.xs}}>Error code: {session.diagnosticCode}</AppText> : null}
          </View>
          {messaging.status !== 'offline' ? <Button title="Reconnect" variant="ghost" size="sm" loading={resuming} onPress={() => { resume().catch(() => {}); }} style={{marginLeft: theme.spacing.sm}} /> : null}
        </View> : null}
        {blocked || !registered || conversation.kind !== 'direct' ? <AppText variant="caption" color="textMuted" accessibilityLiveRegion="polite" style={[styles.centeredText, {marginBottom: theme.spacing.sm}]}>
          {blocked ? 'You blocked this person.' : !registered ? 'Create an account to send messages.' : 'Group messaging is not available yet.'}
        </AppText> : null}
        <View style={styles.composerRow}>
          <TextInput accessibilityLabel="Message" placeholder={blocked ? 'Person blocked' : 'Message'} value={draft} onChangeText={setDraft}
            editable={canCompose && !sending} multiline maxLength={16000} containerStyle={styles.flex}
            style={[styles.input, {paddingHorizontal: theme.spacing.lg, paddingTop: theme.spacing.md, paddingBottom: theme.spacing.md}]} />
          <Button title="↑" accessibilityLabel="Send message" loading={sending} disabled={!canCompose || !draft.trim()} onPress={() => { send().catch(() => {}); }}
            style={[styles.sendButton, {marginLeft: theme.spacing.sm}]} textStyle={styles.sendText} />
        </View>
      </View>
    </ChatKeyboardLayout>
  </Screen>;
}
const styles = StyleSheet.create({
  flex: {flex: 1}, header: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth},
  messages: {flexGrow: 1}, empty: {flex: 1, justifyContent: 'center', alignItems: 'center', minHeight: 160},
  day: {alignSelf: 'center'},
  messageGroup: {maxWidth: '86%'},
  bubbleRow: {flexDirection: 'row', alignItems: 'center', maxWidth: '100%'},
  bubble: {flexShrink: 1, maxWidth: '100%'},
  outgoing: {alignSelf: 'flex-end', alignItems: 'flex-end'},
  incoming: {alignSelf: 'flex-start', alignItems: 'flex-start'},
  metadata: {alignSelf: 'flex-end', flexDirection: 'row', alignItems: 'center', gap: 4, flexWrap: 'wrap', justifyContent: 'flex-end'},
  deliveryIcon: {width: 16, height: 16, borderRadius: 8, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center', transform: [{scale: 0.72}]},
  mailIcon: {width: 19, height: 16, transform: [{scale: 0.72}]},
  mailOutline: {position: 'absolute', left: 0, top: 2, width: 15, height: 11, borderWidth: 1.5, borderRadius: 2},
  mailFlapLeft: {position: 'absolute', left: 1, top: 5, width: 8, height: 1.5, transform: [{rotate: '33deg'}]},
  mailFlapRight: {position: 'absolute', left: 6, top: 5, width: 8, height: 1.5, transform: [{rotate: '-33deg'}]},
  mailCheckBackground: {position: 'absolute', right: 0, bottom: 0, width: 10, height: 9, alignItems: 'center', justifyContent: 'center'},
  mailCheck: {width: 7, height: 4, borderLeftWidth: 1.5, borderBottomWidth: 1.5, transform: [{rotate: '-45deg'}]},
  deliveryMark: {borderLeftWidth: 1.5, borderBottomWidth: 1.5},
  clockHands: {width: 4, height: 4, transform: [{translateX: 1}, {translateY: -1}]},
  checkmark: {width: 7, height: 4, transform: [{rotate: '-45deg'}]},
  exclamation: {alignItems: 'center', gap: 1.5},
  exclamationBar: {width: 1.5, height: 4, borderRadius: 1},
  exclamationDot: {width: 1.5, height: 1.5, borderRadius: 1},
  failureIndicator: {width: 20, height: 20, borderRadius: 10, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center'},
  failureMark: {fontSize: 13, lineHeight: 17, fontWeight: '700'},
  composerRow: {flexDirection: 'row', alignItems: 'flex-end'},
  offlineNotice: {position: 'absolute', zIndex: 1},
  paused: {flexDirection: 'row', alignItems: 'center'},
  timestamp: {fontVariant: ['tabular-nums'], textAlign: 'right'}, deletedText: {opacity: 0.6}, name: {fontWeight: '600'},
  centeredText: {textAlign: 'center'}, centeredSelf: {alignSelf: 'center'}, input: {maxHeight: 140, minHeight: 46, borderRadius: 24},
  sendButton: {width: 46, minHeight: 46, borderRadius: 23, paddingHorizontal: 0}, sendText: {fontSize: 24},
});
