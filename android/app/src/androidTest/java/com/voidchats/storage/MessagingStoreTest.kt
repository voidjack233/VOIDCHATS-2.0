package com.voidchats.storage

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Real SQLCipher/Rust integration using synthetic accounts and an in-memory delivery queue. */
@RunWith(AndroidJUnit4::class)
class MessagingStoreTest {
  private val context = InstrumentationRegistry.getInstrumentation().targetContext
  private val audience = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz"
  private val timestamp = "2026-11-02T06:00:00Z"
  private val expires = "2027-11-02T06:00:00Z"
  private fun uuid() = UUID.randomUUID().toString()
  private fun fixture(voidId: String): Pair<AppStorage, JSONObject> {
    val storage = AppStorage(context, "messaging-test-${uuid()}")
    storage.prepareAccountRegistration(audience)
    storage.requireDatabase().execSQL("UPDATE account_registration SET proof_signed=1,created_at=? WHERE slot=1", arrayOf(timestamp))
    storage.requireDatabase().execSQL("UPDATE local_identity SET account_void_id=? WHERE slot=1", arrayOf(voidId))
    return Pair(storage, JSONObject().put("void_id", voidId).put("auth_public_key", storage.localIdentity()!!.getString("signaturePublicKeyBase64")))
  }
  private data class Accounts(val alice: AppStorage, val bob: AppStorage, val a: JSONObject, val b: JSONObject)
  private fun accounts(): Accounts {
    val (alice, a) = fixture("110559210000000001")
    try {
      val (bob, b) = fixture("110559210000000002")
      return Accounts(alice, bob, a, b)
    } catch (error: Throwable) { alice.removeLocalData(); throw error }
  }
  private fun cleanup(pair: Accounts) { pair.alice.removeLocalData(); pair.bob.removeLocalData() }
  private fun pendingOrNull(store: MessagingStore, purpose: String): JSONObject? {
    val operations = store.state().getJSONArray("outbox")
    for (index in 0 until operations.length()) {
      val row = operations.getJSONObject(index)
      if (row.getString("purpose") == purpose && row.getString("status") == "pending") {
        assertEquals("All current packets use durable send", "message", row.getString("kind"))
        return JSONObject(row.toString())
      }
    }
    return null
  }
  private fun pending(store: MessagingStore, purpose: String = "setup") = pendingOrNull(store, purpose) ?: throw AssertionError("No pending $purpose event")
  private fun accepted(operation: JSONObject) = JSONObject().put("type", "accepted").put("event_id", operation.getString("id"))
    .put("created_at", timestamp).put("expires_at", expires)
  private fun event(operation: JSONObject, sender: JSONObject) = JSONObject().put("type", "event").put("event_id", operation.getString("id"))
    .put("from_void_id", sender.getString("void_id")).put("payload_base64", operation.getString("payloadBase64"))
    .put("created_at", timestamp).put("expires_at", expires)
  private fun acceptPending(store: MessagingStore, purpose: String = "setup"): JSONObject = pending(store, purpose).also {
    store.completeOperation(it.getString("id"), accepted(it))
  }
  private fun applySaved(destination: MessagingStore, operation: JSONObject, sender: JSONObject): JSONObject {
    val ack = destination.applyEvent(event(operation, sender), sender)
    assertEquals(sender.getString("void_id"), ack.getString("sender_void_id"))
    assertEquals(operation.getString("id"), ack.getString("event_id"))
    assertTrue("A valid fixture packet must not be rejected", ack.isNull("diagnosticCode"))
    return ack
  }
  private fun deliver(source: MessagingStore, destination: MessagingStore, sender: JSONObject, purpose: String = "setup"): JSONObject {
    val operation = acceptPending(source, purpose)
    applySaved(destination, operation, sender)
    return operation
  }
  private fun payload(operation: JSONObject): JSONObject {
    val bytes = MessagingProtocol.bytes(operation.getString("payloadBase64"))
    return try { JSONObject(bytes.toString(Charsets.UTF_8)) } finally { bytes.fill(0) }
  }
  private fun session(store: MessagingStore, id: String): JSONObject {
    val sessions = store.state().getJSONArray("sessions")
    return (0 until sessions.length()).map { sessions.getJSONObject(it) }.single { it.getString("id") == id }
  }
  private fun message(storage: AppStorage, id: String) = JSONObject(storage.execute("messages.get", JSONObject().put("id", id)))
  private fun messages(storage: AppStorage, conversationId: String) = JSONObject(storage.execute("messages.page",
    JSONObject().put("conversationId", conversationId).put("limit", 50).put("before", JSONObject.NULL))).getJSONArray("items")
  private fun pump(pair: Accounts) {
    val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
    repeat(24) {
      var progress = false
      for ((source, destination, sender) in listOf(Triple(left, right, pair.a), Triple(right, left, pair.b))) {
        val operation = pendingOrNull(source, "setup") ?: pendingOrNull(source, "application")
        if (operation != null) {
          source.completeOperation(operation.getString("id"), accepted(operation))
          applySaved(destination, operation, sender)
          progress = true
        }
      }
      if (!progress) return
    }
    throw AssertionError("Automatic fixture delivery did not settle")
  }
  private data class Established(val accounts: Accounts, val conversation: String, val firstMessageId: String)
  private fun established(): Established {
    val pair = accounts()
    try {
      val left = MessagingStore(pair.alice)
      pair.alice.execute("contacts.save", JSONObject().put("id", "existing-bob-contact").put("voidId", pair.b.getString("void_id"))
        .put("displayName", "Bob's local name").put("identityPublicKey", pair.b.getString("auth_public_key")).put("createdAt", 1).put("updatedAt", 1))
      val conversation = left.openConversation(pair.b).getString("id")
      assertEquals(0, left.state().getJSONArray("outbox").length())
      left.queueMessage(conversation, "First automatic hello")
      val first = messages(pair.alice, conversation).getJSONObject(0).getString("id")
      pump(pair)
      assertEquals("active", session(left, conversation).getString("status"))
      assertEquals("active", session(MessagingStore(pair.bob), conversation).getString("status"))
      assertEquals("First automatic hello", message(pair.bob, first).getString("body"))
      return Established(pair, conversation, first)
    } catch (error: Throwable) { cleanup(pair); throw error }
  }

  @Test fun firstTextSurvivesAlternatingConnectionsAndSetupAcceptanceIsNotMessageAcceptance() {
    val pair = accounts()
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      pair.bob.lock()
      val conversation = left.openConversation(pair.b).getString("id")
      assertEquals(0, left.state().getJSONArray("outbox").length())
      left.queueMessage(conversation, "Pending private text 世界\nDo not expose in setup")
      val firstMessage = messages(pair.alice, conversation).getJSONObject(0)
      val messageId = firstMessage.getString("id")
      assertEquals("pending", firstMessage.getString("deliveryState"))
      val request = acceptPending(left)
      assertEquals("key_package_request", payload(request).getString("kind"))
      assertFalse(payload(request).has("body"))
      assertFalse(payload(request).toString().contains("Pending private text"))
      assertEquals("pending", message(pair.alice, messageId).getString("deliveryState"))
      pair.alice.lock()

      pair.bob.open()
      applySaved(right, request, pair.a)
      assertFalse(session(right, conversation).getBoolean("hasMessages"))
      val packageReply = acceptPending(right)
      assertEquals("key_package", payload(packageReply).getString("kind"))
      pair.bob.lock()

      pair.alice.open()
      applySaved(left, packageReply, pair.b)
      val welcome = acceptPending(left)
      assertEquals("welcome", payload(welcome).getString("kind"))
      assertEquals("pending", message(pair.alice, messageId).getString("deliveryState"))
      pair.alice.lock()

      pair.bob.open()
      applySaved(right, welcome, pair.a)
      val ready = acceptPending(right)
      assertEquals("ready", payload(ready).getString("kind"))
      pair.bob.lock()

      pair.alice.open()
      applySaved(left, ready, pair.b)
      val confirmation = acceptPending(left)
      assertEquals("ready_ack", payload(confirmation).getString("kind"))
      val encrypted = pending(left, "application")
      assertEquals(messageId, encrypted.getString("id"))
      assertFalse(payload(encrypted).has("body"))
      assertTrue(payload(encrypted).has("ciphertext_base64"))
      assertEquals("pending", message(pair.alice, messageId).getString("deliveryState"))
      left.completeOperation(messageId, accepted(encrypted))
      assertEquals("sent", message(pair.alice, messageId).getString("deliveryState"))
      pair.alice.lock()

      pair.bob.open()
      applySaved(right, confirmation, pair.a)
      applySaved(right, encrypted, pair.a)
      assertEquals("Pending private text 世界\nDo not expose in setup", message(pair.bob, messageId).getString("body"))
      assertTrue(session(right, conversation).getBoolean("isRequest"))
      pair.bob.lock(); pair.bob.open()
      assertFalse(right.applyEvent(event(encrypted, pair.a), pair.a).getBoolean("changed"))
      assertEquals(1, messages(pair.bob, conversation).length())
      pair.alice.open()
      assertEquals(pair.a.getString("auth_public_key"), pair.alice.localIdentity()!!.getString("signaturePublicKeyBase64"))
      assertEquals(pair.b.getString("auth_public_key"), pair.bob.localIdentity()!!.getString("signaturePublicKeyBase64"))
    } finally { cleanup(pair) }
  }

  @Test fun incomingFirstMessageCanBeReadAndRepliedToWithoutApproval() {
    val established = established(); val pair = established.accounts
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      assertTrue(session(right, established.conversation).getBoolean("isRequest"))
      assertTrue(session(right, established.conversation).getBoolean("hasMessages"))
      assertEquals("First automatic hello", message(pair.bob, established.firstMessageId).getString("body"))
      right.queueMessage(established.conversation, "Reply directly from Requests")
      assertFalse(session(right, established.conversation).getBoolean("isRequest"))
      assertNull(pendingOrNull(right, "setup"))
      val reply = deliver(right, left, pair.b, "application")
      assertEquals("Reply directly from Requests", message(pair.alice, reply.getString("id")).getString("body"))
      val contact = JSONObject(pair.alice.execute("contacts.get", JSONObject().put("id", "existing-bob-contact")))
      assertEquals("Bob's local name", contact.getString("displayName"))
      assertEquals("unverified", contact.getString("trustState"))
    } finally { cleanup(pair) }
  }

  @Test fun establishedGroupAndExactCiphertextSurviveBothRestarts() {
    val established = established(); val pair = established.accounts
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      left.queueMessage(established.conversation, "Offline follow-up 世界")
      val outgoing = pending(left, "application")
      pair.alice.lock(); pair.bob.lock(); pair.alice.open()
      assertEquals(outgoing.getString("payloadBase64"), pending(left, "application").getString("payloadBase64"))
      assertNull(pendingOrNull(left, "setup"))
      left.completeOperation(outgoing.getString("id"), accepted(outgoing))
      pair.alice.lock(); pair.bob.open()
      assertTrue(applySaved(right, outgoing, pair.a).getBoolean("changed"))
      assertFalse(right.applyEvent(event(outgoing, pair.a), pair.a).getBoolean("changed"))
      assertEquals("Offline follow-up 世界", message(pair.bob, outgoing.getString("id")).getString("body"))
      pair.bob.lock(); pair.bob.open()
      assertEquals(2, messages(pair.bob, established.conversation).length())
      pair.alice.open()
      assertEquals("sent", message(pair.alice, outgoing.getString("id")).getString("deliveryState"))
    } finally { cleanup(pair) }
  }

  @Test fun journalRecoversIfVisibleHistoryWriteFailsAfterDecryption() {
    val established = established(); val pair = established.accounts
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      left.queueMessage(established.conversation, "Recover after native checkpoint")
      val outgoing = acceptPending(left, "application"); val incoming = event(outgoing, pair.a)
      pair.bob.requireDatabase().execSQL("CREATE TRIGGER test_interrupted_history BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT, 'simulated_history_interruption'); END;")
      var returnedAck = false
      try { right.applyEvent(incoming, pair.a); returnedAck = true } catch (_: Exception) { }
      assertFalse("No ACK may return before history commits", returnedAck)
      assertEquals("null", pair.bob.execute("messages.get", JSONObject().put("id", outgoing.getString("id"))))
      pair.bob.requireDatabase().rawQuery("SELECT outcome,journal_id FROM messaging_inbox WHERE event_id=?", arrayOf(outgoing.getString("id"))).use {
        assertTrue(it.moveToFirst()); assertEquals("staged", it.getString(0)); assertFalse(it.isNull(1))
      }
      pair.bob.requireDatabase().execSQL("DROP TRIGGER test_interrupted_history")
      pair.bob.lock(); pair.bob.open(); right.state()
      assertEquals("Recover after native checkpoint", message(pair.bob, outgoing.getString("id")).getString("body"))
      assertFalse(right.applyEvent(incoming, pair.a).getBoolean("changed"))
      assertEquals(2, messages(pair.bob, established.conversation).length())
    } finally { cleanup(pair) }
  }

  @Test fun wrongRegisteredKeyPackageAndChangedEventReplayAreRejected() {
    val pair = accounts(); val (mallory, _) = fixture("110559210000000003")
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      val conversation = left.openConversation(pair.b).getString("id")
      left.queueMessage(conversation, "Must stay private")
      deliver(left, right, pair.a)
      val actual = pending(right)
      val wrongPackage = mallory.requireEngine().generateKeyPackage(mallory.localIdentity()!!.getString("clientHandle"))
      val forgedPayload = payload(actual).put("key_package_base64", MessagingProtocol.base64(wrongPackage))
      val forged = JSONObject(actual.toString()).put("id", uuid()).put("payloadBase64", MessagingProtocol.base64(forgedPayload.toString().toByteArray()))
      val ack = left.applyEvent(event(forged, pair.b), pair.b)
      assertFalse(ack.getBoolean("changed")); assertFalse(ack.isNull("diagnosticCode"))
      pair.alice.requireDatabase().rawQuery("SELECT outcome FROM messaging_inbox WHERE sender_void_id=? AND event_id=?", arrayOf(pair.b.getString("void_id"), forged.getString("id"))).use {
        assertTrue(it.moveToFirst()); assertEquals("rejected", it.getString(0))
      }
      val changedReplay = JSONObject(forged.toString()).put("payloadBase64", actual.getString("payloadBase64"))
      var returnedAck = false
      try { left.applyEvent(event(changedReplay, pair.b), pair.b); returnedAck = true } catch (_: IllegalArgumentException) { }
      assertFalse("Conflicting event reuse cannot receive an ACK token", returnedAck)
      assertEquals("open", JSONObject(pair.alice.status()).getString("status"))
      assertEquals(pair.a.getString("auth_public_key"), pair.alice.localIdentity()!!.getString("signaturePublicKeyBase64"))
      assertEquals("pending", session(left, conversation).getString("status"))
    } finally { cleanup(pair); mallory.removeLocalData() }
  }

  @Test fun simultaneousFirstMessagesPreserveBothLocalThreadsAndDrafts() {
    val pair = accounts()
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      val aliceThread = left.openConversation(pair.b).getString("id")
      val bobThread = right.openConversation(pair.a).getString("id")
      assertNotEquals(aliceThread, bobThread)
      left.queueMessage(aliceThread, "Alice's simultaneous draft")
      right.queueMessage(bobThread, "Bob's simultaneous draft")
      val aliceMessage = messages(pair.alice, aliceThread).getJSONObject(0).getString("id")
      val bobMessage = messages(pair.bob, bobThread).getJSONObject(0).getString("id")
      val aliceRequest = acceptPending(left); val bobRequest = acceptPending(right)
      applySaved(left, bobRequest, pair.b)
      applySaved(right, aliceRequest, pair.a)
      pump(pair)
      assertEquals("active", session(left, aliceThread).getString("status"))
      assertEquals("active", session(right, bobThread).getString("status"))
      assertFalse(session(right, bobThread).getBoolean("isRequest"))
      assertEquals(bobThread, message(pair.bob, bobMessage).getString("conversationId"))
      assertEquals(bobThread, message(pair.bob, aliceMessage).getString("conversationId"))
      assertEquals(aliceThread, message(pair.alice, bobMessage).getString("conversationId"))
      assertEquals("Bob's simultaneous draft", message(pair.alice, bobMessage).getString("body"))
      assertEquals("Alice's simultaneous draft", message(pair.bob, aliceMessage).getString("body"))
      pair.alice.lock(); pair.bob.lock(); pair.alice.open(); pair.bob.open()
      left.queueMessage(aliceThread, "Thread alias survives restart")
      val followup = deliver(left, right, pair.a, "application")
      assertEquals(bobThread, message(pair.bob, followup.getString("id")).getString("conversationId"))
    } finally { cleanup(pair) }
  }

  @Test fun malformedQueuedControlDoesNotPoisonEstablishedStorage() {
    val established = established(); val pair = established.accounts
    try {
      val left = MessagingStore(pair.alice)
      val malformed = JSONObject().put("v", 1).put("kind", "ready").put("conversation_id", established.conversation)
        .put("from_void_id", pair.b.getString("void_id")).put("to_void_id", pair.a.getString("void_id")).put("ciphertext_base64", "AA==")
      val operation = JSONObject().put("id", uuid()).put("payloadBase64", MessagingProtocol.base64(malformed.toString().toByteArray()))
      val ack = left.applyEvent(event(operation, pair.b), pair.b)
      assertFalse(ack.getBoolean("changed")); assertFalse(ack.isNull("diagnosticCode"))
      assertEquals("open", JSONObject(pair.alice.status()).getString("status"))
      assertEquals(pair.a.getString("auth_public_key"), pair.alice.localIdentity()!!.getString("signaturePublicKeyBase64"))
    } finally { cleanup(pair) }
  }

  @Test fun legacyLostReadyAckRecoversWithoutAliceSendingAnApplicationMessage() {
    val pair = accounts()
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      val conversation = left.openConversation(pair.b).getString("id")
      left.queueMessage(conversation, "Leave Alice's application in its outbox")
      deliver(left, right, pair.a)
      deliver(right, left, pair.b)
      deliver(left, right, pair.a)
      val originalReady = pending(right)
      // This models v1.0.2: an online relay created a response record but no
      // durable messaging_inbox row for the ready event.
      right.completeOperation(originalReady.getString("id"), JSONObject().put("type", "online_result")
        .put("exchange_id", originalReady.getString("id")).put("status", "forwarded"))
      left.applyOnline(pair.b.getString("void_id"), originalReady.getString("id"), originalReady.getString("payloadBase64"), pair.b)
      val lostAck = pending(left)
      assertEquals("ready_ack", payload(lostAck).getString("kind"))
      left.completeOperation(lostAck.getString("id"), JSONObject().put("type", "online_result")
        .put("exchange_id", lostAck.getString("id")).put("status", "forwarded"))
      assertEquals("active", session(left, conversation).getString("status"))
      assertEquals("handshaking", session(right, conversation).getString("status"))
      right.queueMessage(conversation, "Bob replies after the upgrade")
      val bobMessage = messages(pair.bob, conversation).getJSONObject(0).getString("id")
      assertNull(pendingOrNull(right, "application"))
      pair.alice.lock(); pair.bob.lock(); pair.bob.open()
      // Equivalent to migration 005 requeueing the last unfinished setup stage.
      pair.bob.requireDatabase().execSQL("UPDATE messaging_operations SET status='pending',error_code=NULL WHERE id=?", arrayOf(originalReady.getString("id")))
      val repeatedReady = acceptPending(right)
      assertEquals(originalReady.getString("payloadBase64"), repeatedReady.getString("payloadBase64"))
      pair.bob.lock(); pair.alice.open()
      applySaved(left, repeatedReady, pair.b)
      val recoveredAck = pending(left)
      assertEquals(lostAck.getString("payloadBase64"), recoveredAck.getString("payloadBase64"))
      assertNotEquals(lostAck.getString("id"), recoveredAck.getString("id"))
      val acceptedAck = acceptPending(left)
      pair.alice.lock(); pair.bob.open()
      applySaved(right, acceptedAck, pair.a)
      assertEquals("active", session(right, conversation).getString("status"))
      assertEquals(bobMessage, pending(right, "application").getString("id"))
      // Alice's application was never sent to Bob; only the exact stored
      // confirmation was replayed to release Bob's pending reply.
      assertEquals(1, messages(pair.bob, conversation).length())
      assertEquals("Bob replies after the upgrade", message(pair.bob, bobMessage).getString("body"))
    } finally { cleanup(pair) }
  }

  @Test fun oneBrokenDraftCannotBlockAnotherChatOrItsRecipientAck() {
    val established = established(); val pair = established.accounts
    val (other, otherPeer) = fixture("110559210000000003")
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      val brokenThread = left.openConversation(otherPeer).getString("id")
      left.queueMessage(brokenThread, "Preserve the broken chat's draft")
      val draftId = messages(pair.alice, brokenThread).getJSONObject(0).getString("id")
      // A recoverable per-chat mismatch is distinct from database corruption.
      // No native group exists for this synthetic thread.
      pair.alice.requireDatabase().execSQL("UPDATE messaging_sessions SET status='active',peer_credential_base64=? WHERE thread_id=?",
        arrayOf(MessagingProtocol.base64("missing-peer-group".toByteArray()), brokenThread))
      left.resumePending()
      assertEquals("open", JSONObject(pair.alice.status()).getString("status"))
      assertFalse(session(left, brokenThread).isNull("diagnosticCode"))
      assertEquals("Preserve the broken chat's draft", message(pair.alice, draftId).getString("body"))
      right.queueMessage(established.conversation, "An unrelated incoming message still commits")
      val reply = deliver(right, left, pair.b, "application")
      assertEquals("An unrelated incoming message still commits", message(pair.alice, reply.getString("id")).getString("body"))
      left.queueMessage(established.conversation, "The healthy chat still sends")
      val healthy = pending(left, "application")
      assertEquals("application", payload(healthy).getString("kind"))
      assertEquals("open", JSONObject(pair.alice.status()).getString("status"))
    } finally { cleanup(pair); other.removeLocalData() }
  }

  @Test fun lostSetupAcceptanceRetriesExactBytesButApplicationAcceptanceStaysUnknown() {
    val pair = accounts()
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      val conversation = left.openConversation(pair.b).getString("id")
      left.queueMessage(conversation, "First text stays pending during setup")
      val firstMessageId = messages(pair.alice, conversation).getJSONObject(0).getString("id")
      val originalSetup = pending(left)
      left.failOperation(originalSetup.getString("id"), "duplicate_event")
      val retry = pending(left)
      assertNotEquals(originalSetup.getString("id"), retry.getString("id"))
      assertEquals(originalSetup.getString("payloadBase64"), retry.getString("payloadBase64"))
      assertEquals("pending", message(pair.alice, firstMessageId).getString("deliveryState"))
      pair.alice.requireDatabase().rawQuery("SELECT status FROM messaging_operations WHERE id=?", arrayOf(originalSetup.getString("id"))).use {
        assertTrue(it.moveToFirst()); assertEquals("done", it.getString(0))
      }
      left.completeOperation(retry.getString("id"), accepted(retry))
      applySaved(right, retry, pair.a)
      val packageReply = pending(right)
      // The original event may still be queued after the sender lost its
      // acceptance. Different transport UUIDs must not create another package.
      applySaved(right, originalSetup, pair.a)
      assertEquals(packageReply.getString("payloadBase64"), pending(right).getString("payloadBase64"))
      pair.bob.requireDatabase().rawQuery("SELECT count(*) FROM key_packages", emptyArray()).use {
        assertTrue(it.moveToFirst()); assertEquals(1, it.getInt(0))
      }
      pump(pair)
      assertEquals("First text stays pending during setup", message(pair.bob, firstMessageId).getString("body"))

      left.queueMessage(conversation, "Application acceptance is uncertain")
      val application = pending(left, "application")
      left.failOperation(application.getString("id"), "duplicate_event")
      assertEquals("failed", message(pair.alice, application.getString("id")).getString("deliveryState"))
      var retried = false
      try { left.retryOperation(application.getString("id")); retried = true } catch (_: IllegalArgumentException) { }
      assertFalse("Application ciphertext cannot be silently sent under a new UUID", retried)
      assertNull(pendingOrNull(left, "application"))
      val failed = left.state().getJSONArray("outbox")
      val retained = (0 until failed.length()).map { failed.getJSONObject(it) }.single { it.getString("id") == application.getString("id") }
      assertEquals("duplicate_event", retained.getString("errorCode"))
      assertEquals(application.getString("payloadBase64"), retained.getString("payloadBase64"))
      pair.alice.requireDatabase().rawQuery("SELECT count(*) FROM messaging_operations WHERE purpose='application' AND session_id=?", arrayOf(conversation)).use {
        assertTrue(it.moveToFirst()); assertEquals(2, it.getInt(0))
      }
    } finally { cleanup(pair) }
  }

  @Test fun anEmptyComposerCannotWinAgainstThePeersActualFirstMessage() {
    val pair = accounts()
    try {
      val left = MessagingStore(pair.alice); val right = MessagingStore(pair.bob)
      // Alice has the lower VOID-ID, but merely looking up Bob and opening a
      // composer must not suppress Bob's real outgoing setup request.
      val aliceEmptyThread = left.openConversation(pair.b).getString("id")
      assertEquals(0, left.state().getJSONArray("outbox").length())
      assertFalse(session(left, aliceEmptyThread).getBoolean("hasMessages"))
      val bobThread = right.openConversation(pair.a).getString("id")
      right.queueMessage(bobThread, "Bob sent first while Alice's composer was empty")
      val bobMessage = messages(pair.bob, bobThread).getJSONObject(0).getString("id")
      deliver(right, left, pair.b)
      assertEquals("incoming", session(left, aliceEmptyThread).getString("direction"))
      assertEquals("handshaking", session(left, aliceEmptyThread).getString("status"))
      assertTrue(session(left, aliceEmptyThread).getBoolean("isRequest"))
      pump(pair)
      assertEquals("active", session(left, aliceEmptyThread).getString("status"))
      assertEquals("active", session(right, bobThread).getString("status"))
      assertEquals(aliceEmptyThread, message(pair.alice, bobMessage).getString("conversationId"))
      assertEquals("Bob sent first while Alice's composer was empty", message(pair.alice, bobMessage).getString("body"))
      assertTrue(session(left, aliceEmptyThread).getBoolean("isRequest"))
      assertEquals(aliceEmptyThread, left.openConversation(pair.b).getString("id"))
      left.queueMessage(aliceEmptyThread, "Reply using the same open composer")
      assertFalse(session(left, aliceEmptyThread).getBoolean("isRequest"))
      val reply = deliver(left, right, pair.a, "application")
      assertEquals(bobThread, message(pair.bob, reply.getString("id")).getString("conversationId"))
      assertEquals(2, messages(pair.alice, aliceEmptyThread).length())
      assertEquals(2, messages(pair.bob, bobThread).length())
    } finally { cleanup(pair) }
  }

  @Test fun nativeAuthValidationBindsV2ChallengeToAccountKeyAudienceAndLiveExpiry() {
    val (alice, a) = fixture("110559210000000001")
    try {
      val store = MessagingStore(alice)
      val expiry = java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", java.util.Locale.US).apply {
        timeZone = java.util.TimeZone.getTimeZone("UTC")
      }.format(java.util.Date(System.currentTimeMillis() + 10_000))
      val fields = listOf("VOID-NOTHINGNESS:authenticate:v2", "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8", a.getString("void_id"), a.getString("auth_public_key"), expiry,
        MessagingProtocol.base64(audience.toByteArray()), "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8", "")
      fun encoded(parts: List<String>) = MessagingProtocol.base64(parts.joinToString("\n").toByteArray())
      assertEquals(64, MessagingProtocol.bytes(store.signAuthChallenge(encoded(fields)), 64).size)
      for ((index, replacement) in listOf(0 to "VOID-NOTHINGNESS:auth:v1", 1 to "invalid-token", 2 to "999", 3 to MessagingProtocol.base64(ByteArray(32)),
        4 to timestamp, 5 to MessagingProtocol.base64("https://other.example".toByteArray()), 6 to "invalid-nonce")) {
        val changed = fields.toMutableList(); changed[index] = replacement
        try { store.signAuthChallenge(encoded(changed)); fail("A mismatched challenge must not be signed") } catch (_: IllegalArgumentException) { }
      }
      assertEquals(a.getString("auth_public_key"), alice.localIdentity()!!.getString("signaturePublicKeyBase64"))
    } finally { alice.removeLocalData() }
  }
}
