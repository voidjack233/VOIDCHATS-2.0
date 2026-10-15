package com.voidchats.storage

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.security.KeyStore
import java.security.MessageDigest
import java.util.UUID
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class EncryptedStorageTest {
  private val context = InstrumentationRegistry.getInstrumentation().targetContext
  private fun namespace() = "instrumentation-${UUID.randomUUID()}"
  private fun failClosed(operation: () -> Any?) {
    try { operation(); fail("Protected data must not open") } catch (_: StorageFailure) { }
  }

  @Test fun sqlCipherEncryptionAndKeystoreRoundTrip() {
    val name = namespace(); val store = EncryptedDatabase(context, name)
    try {
      val db = store.open(create = true)
      db.execSQL("CREATE TABLE encryption_probe(value TEXT NOT NULL)")
      db.execSQL("INSERT INTO encryption_probe(value) VALUES(?)", arrayOf("private-payload-does-not-belong-in-files"))
      store.close()
      val bytes = store.databaseFile.readBytes()
      assertFalse(bytes.copyOfRange(0, 16).contentEquals("SQLite format 3\u0000".toByteArray()))
      assertFalse(bytes.toString(Charsets.ISO_8859_1).contains("private-payload-does-not-belong-in-files"))
      val second = EncryptedDatabase(context, name)
      try { second.open().rawQuery("SELECT value FROM encryption_probe", emptyArray()).use { assertTrue(it.moveToFirst()); assertEquals("private-payload-does-not-belong-in-files", it.getString(0)) } }
      finally { second.close() }
    } finally { store.removeLocalData() }
  }

  @Test fun concurrentInitializersShareOneKeyAndStableLocalIdentity() {
    val name = namespace(); val first = AppStorage(context, name); val second = AppStorage(context, name)
    val workers = java.util.concurrent.Executors.newFixedThreadPool(2)
    val start = java.util.concurrent.CountDownLatch(1)
    try {
      val one = workers.submit(java.util.concurrent.Callable { start.await(); first.initializeDevelopment() })
      val two = workers.submit(java.util.concurrent.Callable { start.await(); second.initializeDevelopment() })
      start.countDown()
      val firstIdentity = JSONObject(one.get(30, java.util.concurrent.TimeUnit.SECONDS)).getJSONObject("identity")
      val secondIdentity = JSONObject(two.get(30, java.util.concurrent.TimeUnit.SECONDS)).getJSONObject("identity")
      assertEquals(firstIdentity.toString(), secondIdentity.toString())
      val wrapped = EncryptedDatabase(context, name).wrappedKeyFile.readBytes()
      first.lock(); second.lock(); first.open(); second.open()
      assertEquals(firstIdentity.toString(), first.localIdentity()!!.toString()); assertEquals(firstIdentity.toString(), second.localIdentity()!!.toString())
      assertArrayEquals(wrapped, EncryptedDatabase(context, name).wrappedKeyFile.readBytes())
    } finally { workers.shutdownNow(); second.lock(); first.removeLocalData() }
  }

  @Test fun removalRefusesAnotherLiveOwnerAndPreservesItsUsableDatabase() {
    val name = namespace(); val first = EncryptedDatabase(context, name); val second = EncryptedDatabase(context, name)
    try {
      first.open(create = true).execSQL("CREATE TABLE retained(value TEXT)")
      second.open().execSQL("INSERT INTO retained(value) VALUES(?)", arrayOf("still protected"))
      val wrapped = first.wrappedKeyFile.readBytes()
      try { first.removeLocalData(); fail("Removal must refuse another active owner") } catch (error: StorageFailure) { assertEquals("OPERATION_FAILED", error.code) }
      assertArrayEquals(wrapped, first.wrappedKeyFile.readBytes()); assertTrue(first.databaseFile.exists())
      val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }; assertTrue(keyStore.containsAlias(first.keyAlias))
      second.open().rawQuery("SELECT value FROM retained", emptyArray()).use { assertTrue(it.moveToFirst()); assertEquals("still protected", it.getString(0)) }
      second.close(); first.removeLocalData()
      assertFalse(first.exists()); assertFalse(first.databaseFile.exists()); assertFalse(first.wrappedKeyFile.exists()); assertFalse(keyStore.containsAlias(first.keyAlias))
    } finally { second.close(); first.removeLocalData() }
  }

  @Test fun missingWrappedKeyNeverResetsExistingDatabase() {
    val store = EncryptedDatabase(context, namespace())
    try {
      store.open(create = true).execSQL("CREATE TABLE retained(value TEXT)"); store.close()
      val before = MessageDigest.getInstance("SHA-256").digest(store.databaseFile.readBytes())
      assertTrue(store.wrappedKeyFile.delete()); failClosed { store.open() }
      assertArrayEquals(before, MessageDigest.getInstance("SHA-256").digest(store.databaseFile.readBytes()))
      assertFalse(store.wrappedKeyFile.exists())
    } finally { store.removeLocalData() }
  }

  @Test fun missingKeystoreKeyNeverGeneratesReplacement() {
    val store = EncryptedDatabase(context, namespace())
    try {
      store.open(create = true).execSQL("CREATE TABLE retained(value TEXT)"); store.close()
      val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
      keyStore.deleteEntry(store.keyAlias)
      failClosed { store.open() }; assertFalse(keyStore.containsAlias(store.keyAlias)); assertTrue(store.databaseFile.exists())
    } finally { store.removeLocalData() }
  }

  @Test fun tamperedWrappedKeyCannotOpenAndDoesNotDeleteDatabase() {
    val store = EncryptedDatabase(context, namespace())
    try {
      store.open(create = true).execSQL("CREATE TABLE retained(value TEXT)"); store.close()
      val length = store.databaseFile.length()
      val wrapped = store.wrappedKeyFile.readBytes(); wrapped[wrapped.lastIndex] = (wrapped.last().toInt() xor 1).toByte(); store.wrappedKeyFile.writeBytes(wrapped)
      failClosed { store.open() }; assertEquals(length, store.databaseFile.length())
    } finally { store.removeLocalData() }
  }

  @Test fun freshOpenDoesNotCreateFilesKeysOrIdentity() {
    val name = namespace(); val store = AppStorage(context, name)
    try {
      val result = JSONObject(store.open())
      assertEquals("not_initialized", result.getString("status")); assertTrue(result.isNull("identity")); assertTrue(result.isNull("errorCode"))
      assertFalse(EncryptedDatabase(context, name).exists())
    } finally { store.removeLocalData() }
  }

  @Test fun corruptDatabaseIsPreservedAndNeverReset() {
    val store = EncryptedDatabase(context, namespace())
    try {
      store.open(create = true).execSQL("CREATE TABLE retained(value TEXT)"); store.close()
      val bytes = store.databaseFile.readBytes(); bytes[64] = (bytes[64].toInt() xor 1).toByte(); store.databaseFile.writeBytes(bytes)
      failClosed { store.open() }; assertArrayEquals(bytes, store.databaseFile.readBytes())
    } finally { store.removeLocalData() }
  }

  @Test fun emptyAndTruncatedExistingFilesArePreservedWithoutMigrations() {
    for (length in listOf(0, 64)) {
      val store = EncryptedDatabase(context, namespace())
      try {
        store.open(create = true).execSQL("CREATE TABLE retained(value TEXT)"); store.close()
        val damaged = store.databaseFile.readBytes().copyOf(length); store.databaseFile.writeBytes(damaged)
        failClosed { store.open() }; assertArrayEquals(damaged, store.databaseFile.readBytes())
      } finally { store.removeLocalData() }
    }
  }

  @Test fun conversationPagesPreserveEqualTimeOrderingAndConstraintErrorsAreConflicts() {
    val store = AppStorage(context, namespace())
    try {
      store.initializeDevelopment()
      for (id in listOf("a", "b", "c")) store.execute("conversations.save", JSONObject().put("id", id).put("kind", "direct").put("title", "Conversation $id").put("peerVoidId", "peer-$id").put("mlsGroupId", JSONObject.NULL).put("createdAt", 1).put("updatedAt", 10))
      val first = JSONObject(store.execute("conversations.page", JSONObject("""{"before":null,"limit":2}""")))
      assertEquals("c", first.getJSONArray("items").getJSONObject(0).getString("id")); assertEquals("b", first.getJSONObject("nextCursor").getString("id"))
      store.lock(); store.open()
      val second = JSONObject(store.execute("conversations.page", JSONObject().put("before", first.getJSONObject("nextCursor")).put("limit", 2)))
      assertEquals("a", second.getJSONArray("items").getJSONObject(0).getString("id")); assertTrue(second.isNull("nextCursor"))
      store.execute("messages.save", JSONObject("""{"id":"outbound","conversationId":"a","senderVoidId":"local","body":"Pending","direction":"outgoing","deliveryState":"pending","createdAt":10}"""))
      try {
        store.execute("messages.setDeliveryState", JSONObject("""{"id":"outbound","state":"read"}""")); fail("Invalid transition must conflict")
      } catch (error: StorageFailure) { assertEquals("CONFLICT", error.code) }
      assertEquals("pending", JSONObject(store.execute("messages.get", JSONObject().put("id", "outbound"))).getString("deliveryState"))
    } finally { store.removeLocalData() }
  }

  @Test fun editedMigrationHistoryAndFutureSchemaFailClosed() {
    for (future in listOf(false, true)) {
      val store = AppStorage(context, namespace())
      try {
        store.initializeDevelopment()
        if (future) store.requireDatabase().version = 100
        else store.requireDatabase().execSQL("UPDATE storage_migrations SET checksum='edited' WHERE version=1")
        store.lock(); failClosed { store.open() }
        assertEquals("error", JSONObject(store.status()).getString("status"))
      } finally { store.removeLocalData() }
    }
  }

  @Test fun largePrivateSnapshotReadsStayNativeAndAvoidCursorWindowLimits() {
    val store = AppStorage(context, namespace())
    try {
      store.initializeDevelopment()
      // A native-only storage capacity probe; this is never presented as a valid MLS state.
      val probe = ByteArray(3 * 1024 * 1024) { (it % 251).toByte() }
      store.requireDatabase().compileStatement("UPDATE native_mls_state SET snapshot=? WHERE slot=1").use { it.bindBlob(1, probe); it.executeUpdateDelete() }
      val loaded = store.loadMlsState()!!
      assertArrayEquals(probe, loaded.snapshot); loaded.snapshot.fill(0); probe.fill(0)
    } finally { store.removeLocalData() }
  }

  @Test fun recoveredDecryptionsAreBoundedAndDoNotAdvanceRatchets() {
    val alice = AppStorage(context, namespace()); val bob = AppStorage(context, namespace())
    try {
      alice.initializeDevelopment(); bob.initializeDevelopment()
      val a = alice.localIdentity()!!.getString("clientHandle"); val b = bob.localIdentity()!!.getString("clientHandle"); val group = "recovery-page-group".toByteArray()
      alice.mutateMls { it.createGroup(a, group) }
      val packageBytes = bob.mutateMls { it.generateKeyPackage(b) }
      val added = alice.mutateMls { it.addMember(a, group, packageBytes) }
      alice.mutateMls { it.mergePendingCommit(a, group) }; bob.mutateMls { it.processWelcome(b, added.welcome) }
      repeat(64) { index ->
        val plaintext = ByteArray(64 * 1024) { index.toByte() }
        val wire = alice.mutateMls { it.encrypt(a, group, plaintext) }
        bob.mutateMls {
          val result = it.decrypt(b, group, wire)
          bob.journal("decrypt", group, result.plaintext, wire, result.senderIdentity, result.senderSignaturePublicKey, result.epoch.toString())
          assertArrayEquals(plaintext, result.plaintext); result.plaintext.fill(0)
        }
        plaintext.fill(0); wire.fill(0)
      }
      val revision = bob.requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use { assertTrue(it.moveToFirst()); it.getLong(0) }
      bob.lock(); bob.open()
      val firstText = bob.readStoredDecryptions("null", 100.0); assertTrue(firstText.length <= 4 * 1024 * 1024)
      val first = JSONObject(firstText); assertTrue(first.getJSONArray("items").length() in 1..63); assertFalse(first.isNull("nextCursor"))
      val second = JSONObject(bob.readStoredDecryptions(first.getJSONObject("nextCursor").toString(), 100.0))
      assertEquals(64, first.getJSONArray("items").length() + second.getJSONArray("items").length()); assertTrue(second.isNull("nextCursor"))
      val after = bob.requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use { assertTrue(it.moveToFirst()); it.getLong(0) }
      assertEquals(revision, after)
      try { bob.readStoredDecryptions("null", 101.0); fail("Recovery limit must be bounded") } catch (_: IllegalArgumentException) { }
      bob.requireDatabase().execSQL("UPDATE crypto_events SET sender_identity=NULL WHERE operation='decrypt'")
      failClosed { bob.readStoredDecryptions("null", 1.0) }
    } finally { alice.removeLocalData(); bob.removeLocalData() }
  }

  @Test fun durableLocalIdentityReopensWithoutRegenerationAndPrivateTablesAreUnavailable() {
    val name = namespace(); val first = AppStorage(context, name)
    try {
      val initialized = JSONObject(first.initializeDevelopment()).getJSONObject("identity")
      assertTrue(initialized.isNull("accountVoidId")); first.lock()
      val second = AppStorage(context, name)
      try {
        assertEquals(initialized.toString(), JSONObject(second.open()).getJSONObject("identity").toString())
        assertTrue(second.requireEngine().engineInfo().durablePersistence)
        try { second.execute("native_mls_state.get", JSONObject()); fail("Private tables must be unavailable") } catch (_: IllegalArgumentException) { }
      } finally { second.lock() }
    } finally { first.removeLocalData() }
  }

  @Test fun historyPaginationUnreadTombstonesAndTrustChangesAreDurable() {
    val store = AppStorage(context, namespace())
    try {
      store.initializeDevelopment()
      store.execute("conversations.save", JSONObject("""{"id":"chat","kind":"direct","title":"Test chat","peerVoidId":"peer-public-id","mlsGroupId":null,"createdAt":1,"updatedAt":1,"members":[]}"""))
      for (id in listOf("a", "b", "c")) store.execute("messages.save", JSONObject().put("id", id).put("conversationId", "chat").put("senderVoidId", "peer-public-id").put("body", "Encrypted history $id").put("direction", "incoming").put("deliveryState", "received").put("createdAt", 10))
      val page = JSONObject(store.execute("messages.page", JSONObject("""{"conversationId":"chat","limit":2,"before":null}""")))
      assertEquals("c", page.getJSONArray("items").getJSONObject(0).getString("id")); assertEquals("b", page.getJSONObject("nextCursor").getString("id"))
      val next = JSONObject(store.execute("messages.page", JSONObject().put("conversationId", "chat").put("limit", 2).put("before", page.getJSONObject("nextCursor"))))
      assertEquals("a", next.getJSONArray("items").getJSONObject(0).getString("id")); assertTrue(next.isNull("nextCursor"))
      store.execute("conversations.markRead", JSONObject("""{"conversationId":"chat","through":{"createdAt":10,"id":"c"},"readAt":20}"""))
      assertEquals(0, JSONObject(store.execute("conversations.get", JSONObject().put("id", "chat"))).getInt("unreadCount"))
      store.execute("messages.save", JSONObject("""{"id":"late","conversationId":"chat","senderVoidId":"peer-public-id","body":"Late payload","direction":"incoming","deliveryState":"received","createdAt":5}"""))
      assertEquals(1, JSONObject(store.execute("conversations.get", JSONObject().put("id", "chat"))).getInt("unreadCount"))
      store.execute("messages.setReaction", JSONObject("""{"id":"c","reactorVoidId":"local","emoji":"+1","active":true,"createdAt":20}"""))
      val deleted = JSONObject(store.execute("messages.delete", JSONObject("""{"id":"c","deletedAt":30}""")))
      assertTrue(deleted.isNull("body")); assertEquals(0, deleted.getJSONArray("reactions").length())
      val contact = JSONObject("""{"id":"contact","voidId":"peer-public-id","displayName":"Peer","identityPublicKey":"public-key-one","createdAt":1,"updatedAt":1}""" )
      assertEquals("unverified", JSONObject(store.execute("contacts.save", contact)).getString("trustState"))
      contact.put("identityPublicKey", "public-key-two").put("updatedAt", 2)
      assertEquals("changed", JSONObject(store.execute("contacts.save", contact)).getString("trustState"))
      store.lock(); store.open()
      assertTrue(JSONObject(store.execute("messages.get", JSONObject().put("id", "c"))).isNull("body"))
      assertEquals("changed", JSONObject(store.execute("contacts.get", JSONObject().put("id", "contact"))).getString("trustState"))
    } finally { store.removeLocalData() }
  }
}
