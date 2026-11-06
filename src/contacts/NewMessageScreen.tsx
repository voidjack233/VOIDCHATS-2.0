import React, {useEffect, useRef, useState} from 'react';
import {ScrollView, View} from 'react-native';
import {isVoidId} from '../account/validation';
import {conversationFromRecord, type Conversation} from '../conversations/types';
import {useMessaging} from '../messaging';
import {useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Button, IconButton, Screen, TextInput} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export function NewMessageScreen({onBack, onOpenConversation}: {onBack: () => void; onOpenConversation: (conversation: Conversation) => void}) {
  const theme = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const [voidId, setVoidId] = useState('');
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const mounted = useRef(true);
  const scope = storage.status === 'ready' ? storage.identity?.localDeviceId : undefined;
  const currentScope = useRef(scope); currentScope.current = scope;
  const self = storage.status === 'ready' ? storage.identity?.accountVoidId : null;
  const valid = isVoidId(voidId) && voidId !== self;

  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setVoidId(''); setError(null); setWorking(false); busy.current = false; }, [scope]);

  async function openChat() {
    if (!self || !valid || busy.current) { return; }
    if (messaging.blockedVoidIds.includes(voidId)) { setError('This person is blocked.'); return; }
    busy.current = true; setWorking(true); setError(null);
    const active = () => mounted.current && currentScope.current === scope;
    try {
      const peer = await messaging.lookup(voidId);
      if (!active()) { return; }
      const conversation = await messaging.openConversation(peer);
      if (active()) { onOpenConversation(conversationFromRecord(conversation)); }
    } catch (value) {
      if (active()) { setError(value instanceof Error ? value.message : 'Could not open this chat. Try again.'); }
    } finally { if (active()) { busy.current = false; setWorking(false); } }
  }

  return <Screen>
    <View style={{flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.spacing.xl, paddingTop: theme.spacing.xl, paddingBottom: theme.spacing.xl}}>
        <IconButton accessibilityLabel="Back to messages" onPress={onBack} icon={<AppText variant="title">‹</AppText>} />
        <AppText variant="title" style={{marginLeft: theme.spacing.sm}}>New message</AppText>
    </View>
    <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xl}}>
      <StorageStatus />
      <TextInput label="VOID-ID" placeholder="Enter a VOID-ID" value={voidId}
        onChangeText={value => { setVoidId(value); setError(null); }} keyboardType="number-pad"
        autoCapitalize="none" autoCorrect={false} maxLength={19} editable={Boolean(self) && !working}
        returnKeyType="go" onSubmitEditing={() => { openChat().catch(() => {}); }}
        error={voidId === self ? 'Enter someone else’s VOID-ID.' : voidId && !isVoidId(voidId) ? 'Enter a valid VOID-ID.' : undefined} />
      <Button title="Message" fullWidth loading={working} disabled={!self || !valid} onPress={() => { openChat().catch(() => {}); }} style={{marginTop: theme.spacing.md}} />
      {!self ? <AppText color="textMuted" style={{marginTop: theme.spacing.md}}>Create your account to send messages.</AppText> : null}
      {error ? <AppText color="danger" accessibilityLiveRegion="polite" style={{marginTop: theme.spacing.md}}>{error}</AppText> : null}
    </ScrollView>
  </Screen>;
}
