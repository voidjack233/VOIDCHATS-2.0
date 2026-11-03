import React, {useEffect, useRef, useState} from 'react';
import {FlatList, Pressable, StyleSheet, View} from 'react-native';
import {isVoidId} from '../account/validation';
import {conversationFromRecord, type Conversation} from '../conversations/types';
import {useMessaging} from '../messaging';
import {useContacts, useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Avatar, Button, Screen, TextInput} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export interface ContactsScreenProps {onOpenConversation: (conversation: Conversation) => void}

export function ContactsScreen({onOpenConversation}: ContactsScreenProps) {
  const theme = useTheme();
  const storage = useStorage();
  const messaging = useMessaging();
  const contacts = useContacts();
  const [voidId, setVoidId] = useState('');
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);
  const mounted = useRef(true);
  const scope = storage.identity?.localDeviceId;
  const currentScope = useRef(scope); currentScope.current = scope;
  const self = storage.status === 'ready' ? storage.identity?.accountVoidId : null;
  const registered = Boolean(self);
  const valid = isVoidId(voidId) && voidId !== self;

  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setVoidId(''); setError(null); setWorking(false); busy.current = false; }, [scope]);

  async function message(id: string, publicKey?: string) {
    if (!registered || busy.current || id === self || messaging.blockedVoidIds.includes(id)) { return; }
    busy.current = true; setWorking(true); setError(null);
    const active = () => mounted.current && currentScope.current === scope;
    try {
      const peer = publicKey ? {void_id: id, auth_public_key: publicKey} : await messaging.lookup(id);
      if (!active()) { return; }
      const conversation = await messaging.openConversation(peer);
      if (active()) { onOpenConversation(conversationFromRecord(conversation)); }
    } catch (value) {
      if (active()) { setError(value instanceof Error ? value.message : 'Could not open this chat. Try again.'); }
    } finally { if (active()) { busy.current = false; setWorking(false); } }
  }

  return <Screen>
    <FlatList
      data={contacts.items}
      keyExtractor={item => item.id}
      keyboardShouldPersistTaps="handled"
      refreshing={contacts.loading && storage.status === 'ready'}
      onRefresh={contacts.refresh}
      contentContainerStyle={{padding: theme.spacing.xl, paddingBottom: theme.spacing.xxxl}}
      ListHeaderComponent={<>
        <AppText variant="title">New message</AppText>
        <AppText color="textSecondary" style={{marginTop: theme.spacing.sm, marginBottom: theme.spacing.xl}}>Who would you like to message?</AppText>
        <StorageStatus />
        <TextInput
          label="VOID-ID"
          placeholder="Enter a VOID-ID"
          value={voidId}
          onChangeText={value => { setVoidId(value); setError(null); }}
          keyboardType="number-pad"
          autoCapitalize="none"
          autoCorrect={false}
          maxLength={19}
          editable={registered && !working}
          returnKeyType="go"
          onSubmitEditing={() => { if (valid) { message(voidId).catch(() => {}); } }}
          error={voidId === self ? 'Enter someone else’s VOID-ID.' : voidId && !isVoidId(voidId) ? 'Enter a valid VOID-ID.' : undefined}
        />
        <Button title="Message" fullWidth loading={working} disabled={!registered || !valid} onPress={() => { message(voidId).catch(() => {}); }} style={{marginTop: theme.spacing.md}} />
        {!registered ? <AppText color="textMuted" style={{marginTop: theme.spacing.md}}>Create your account to send messages.</AppText> : null}
        {error ? <AppText color="danger" accessibilityLiveRegion="polite" style={{marginTop: theme.spacing.md}}>{error}</AppText> : null}
        <AppText variant="label" color="textSecondary" style={{marginTop: theme.spacing.xxl, marginBottom: theme.spacing.md}}>Contacts</AppText>
        {contacts.error ? <View style={{marginBottom: theme.spacing.lg}}><AppText color="danger">Could not load contacts.</AppText><Button title="Try again" variant="ghost" onPress={contacts.refresh} /></View> : null}
      </>}
      renderItem={({item}) => {
        const blocked = messaging.blockedVoidIds.includes(item.voidId);
        return <Pressable accessibilityRole="button" accessibilityLabel={`Message ${item.displayName}`} disabled={!registered || working || blocked}
          onPress={() => { message(item.voidId, item.identityPublicKey).catch(() => {}); }}
          style={({pressed}) => [styles.row, {paddingVertical: theme.spacing.lg, borderBottomColor: theme.colors.border, opacity: pressed || blocked ? 0.55 : 1}]}>
          <Avatar name={item.displayName} size={44} />
          <View style={[styles.flex, {marginLeft: theme.spacing.md}]}>
            <AppText variant="body" numberOfLines={1} style={styles.name}>{item.displayName === `VOID-${item.voidId}` ? item.voidId : item.displayName}</AppText>
            <AppText variant="caption" color="textSecondary" style={{marginTop: theme.spacing.xs}}>{blocked ? 'Blocked' : item.voidId}</AppText>
          </View>
          {!blocked ? <AppText color="textMuted">›</AppText> : null}
        </Pressable>;
      }}
      ListEmptyComponent={storage.status === 'ready' && !contacts.error ? <AppText color="textMuted" style={{paddingVertical: theme.spacing.lg}}>{contacts.loading ? 'Loading contacts…' : 'Enter a VOID-ID above to start a chat.'}</AppText> : undefined}
    />
  </Screen>;
}
const styles = StyleSheet.create({row: {flexDirection: 'row', alignItems: 'center', borderBottomWidth: StyleSheet.hairlineWidth}, flex: {flex: 1}, name: {fontWeight: '600'}});
