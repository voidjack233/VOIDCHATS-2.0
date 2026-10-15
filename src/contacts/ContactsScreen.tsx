import React from 'react';
import {FlatList, StyleSheet, View} from 'react-native';
import {useContacts, useStorage} from '../storage';
import {useTheme} from '../theme';
import {AppText, Avatar, Button, Screen, Surface} from '../ui';
import {StorageStatus} from '../ui/StorageStatus';

export function ContactsScreen() {
  const theme = useTheme();
  const storage = useStorage();
  const {items, loading, error, refresh} = useContacts();
  return (
    <Screen>
      <FlatList
        data={items}
        keyExtractor={item => item.id}
        refreshing={loading && storage.status === 'ready'}
        onRefresh={() => { refresh(); }}
        contentContainerStyle={{paddingHorizontal: theme.spacing.xl, paddingBottom: theme.spacing.xxl}}
        ListHeaderComponent={<>
          <AppText variant="title" style={{marginTop: theme.spacing.xl}}>Contacts</AppText>
          <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>Public identity information saved on this device.</AppText>
          <StorageStatus />
          <AppText variant="caption" color="textMuted" style={[styles.sectionLabel, {marginTop: theme.spacing.xxl, marginBottom: theme.spacing.sm}]}>SAVED CONTACTS</AppText>
          {error && storage.status === 'ready' ? <View><AppText variant="body" color="danger">Could not load contacts.</AppText><Button title="Try again" variant="secondary" onPress={() => { refresh(); }} style={{marginTop: theme.spacing.md}} /></View> : null}
        </>}
        renderItem={({item}) => <Surface bordered padded style={{marginBottom: theme.spacing.md}}>
          <View style={styles.row}>
            <Avatar name={item.displayName} size={40} />
            <View style={[styles.flex, {marginLeft: theme.spacing.md}]}>
              <AppText variant="label">{item.displayName}</AppText>
              <AppText variant="caption" color="textSecondary" selectable>{item.voidId}</AppText>
            </View>
          </View>
          <AppText variant="caption" color={item.trustState === 'changed' ? 'warning' : 'textMuted'} style={{marginTop: theme.spacing.md}}>
            {item.trustState === 'changed' ? 'Public key changed · Identity unverified' : 'Identity unverified'}
          </AppText>
          <AppText variant="caption" color="textSecondary" numberOfLines={3} selectable style={{marginTop: theme.spacing.xs}}>{item.identityPublicKey}</AppText>
        </Surface>}
        ListEmptyComponent={storage.status === 'ready' && !error ? <View style={[styles.emptyState, {paddingVertical: theme.spacing.xl}]}>
          <AppText variant="heading">{loading ? 'Loading contacts…' : 'No contacts yet'}</AppText>
          <AppText variant="body" color="textSecondary" style={[styles.emptyDescription, {marginTop: theme.spacing.sm}]}>No contacts are saved on this device yet.</AppText>
        </View> : null}
        ListFooterComponent={<Surface bordered padded style={{marginTop: theme.spacing.xxl}}>
          <AppText variant="heading">Find someone by VOID-ID</AppText>
          <AppText variant="body" color="textSecondary" style={{marginTop: theme.spacing.sm}}>VOID-ID lookup is not connected yet. A saved public key does not establish trust.</AppText>
          <Button title="Search VOID-ID" variant="secondary" size="sm" disabled style={{marginTop: theme.spacing.lg}} />
        </Surface>}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  sectionLabel: {letterSpacing: 1},
  row: {flexDirection: 'row', alignItems: 'center'},
  flex: {flex: 1},
  emptyState: {alignItems: 'center'},
  emptyDescription: {textAlign: 'center'},
});
