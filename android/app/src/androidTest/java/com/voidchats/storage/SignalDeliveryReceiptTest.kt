package com.voidchats.storage

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.time.Instant
import java.util.UUID
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.signal.libsignal.protocol.SessionCipher
import org.signal.libsignal.protocol.SignalProtocolAddress

/** Real libsignal and SQLCipher using unique, disposable account namespaces. */
@RunWith(AndroidJUnit4::class)
class SignalDeliveryReceiptTest {
  private val context = InstrumentationRegistry.getInstrumentation().targetContext
  private val audience = "https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz"
  private fun uuid() = UUID.randomUUID().toString()

  private inner class Device(val voidId: String) {
    private val namespace = "signal-receipt-test-${uuid()}"
    var storage = AppStorage(context, namespace)
      private set
    val person: JSONObject

    init {
      try {
        storage.prepareAccountRegistration(audience)
        storage.transaction {
          storage.requireDatabase().execSQL("UPDATE account_registration SET proof_signed=1,created_at=? WHERE slot=1", arrayOf(Instant.now().toString()))
          storage.requireDatabase().execSQL("UPDATE local_identity SET account_void_id=? WHERE slot=1", arrayOf(voidId))
        }
        person = JSONObject().put("void_id", voidId)
          .put("auth_public_key", storage.localIdentity()!!.getString("signaturePublicKeyBase64"))
      } catch (error: Throwable) {
        storage.removeLocalData()
        throw error
      }
    }

    fun messaging(request: Boolean = false) = SignalMessagingStore(storage, requestDeliveryReceipts = request)
    fun reopen() {
      storage.lock()
      storage = AppStorage(context, namespace)
      val status = JSONObject(storage.open())
      assertEquals("open", status.getString("status"))
      assertEquals(voidId, status.getJSONObject("identity").getString("accountVoidId"))
      assertEquals(person.getString("auth_public_key"), status.getJSONObject("identity").getString("signaturePublicKeyBase64"))
    }
    fun close() { storage.removeLocalData() }
  }

  private fun withPeers(action: (Device, Device) -> Unit) {
    val alice = Device("110559210000000011")
    try {
      val bob = Device("110559210000000012")
      try { action(alice, bob) } finally { bob.close() }
    } finally { alice.close() }
  }

  private fun value(device: Device, sql: String, vararg args: String): String? =
    device.storage.requireDatabase().rawQuery(sql, args).use { if (it.moveToFirst() && !it.isNull(0)) it.getString(0) else null }

  private fun frame(type: String, id: String) = JSONObject().put("type", type).put("event_id", id)
    .put("created_at", Instant.now().toString()).put("expires_at", Instant.now().plusSeconds(86_400).toString())

  private fun event(operation: JSONObject, sender: Device) = frame("event", operation.getString("id"))
    .put("from_void_id", sender.voidId).put("payload_base64", operation.getString("payloadBase64"))

  private fun apply(destination: Device, operation: JSONObject, sender: Device): JSONObject =
    destination.messaging().applyEvent(event(operation, sender), sender.person).also {
      assertEquals(sender.voidId, it.getString("sender_void_id"))
      assertEquals(operation.getString("id"), it.getString("event_id"))
    }

  private fun establish(sender: Device, recipient: Device): String {
    val publication = recipient.messaging().getPreKeyPublication() as JSONObject
    val bundle = JSONObject(MessagingProtocol.bytes(publication.getString("payloadBase64")).toString(Charsets.UTF_8))
    bundle.put("claim_expires_at", Instant.now().plusSeconds(3600).toString())
      .put("one_time_prekey", bundle.getJSONArray("one_time_prekeys").getJSONObject(0))
      .put("kyber_prekey", bundle.getJSONArray("one_time_kyber_prekeys").getJSONObject(0))
    val store = sender.messaging()
    val conversation = store.openConversation(recipient.person).getString("id")
    store.establishSignalSession(recipient.person, MessagingProtocol.base64(bundle.toString().toByteArray(Charsets.UTF_8)))
    return conversation
  }

  private fun queue(device: Device, conversation: String, body: String, request: Boolean = false): JSONObject {
    val operations = device.messaging(request).queueMessage(conversation, body).getJSONArray("outbox")
    val id = value(device, "SELECT id FROM messages WHERE conversation_id=? AND body=? AND direction='outgoing'", conversation, body)
    return (0 until operations.length()).map { operations.getJSONObject(it) }.single { it.getString("id") == id }
  }

  private fun state(device: Device, messageId: String) = value(device, "SELECT delivery_state FROM messages WHERE id=?", messageId)

  private fun message(device: Device, id: String) = JSONObject(device.storage.execute("messages.get", JSONObject().put("id", id)))

  private fun ratchet(device: Device, peer: Device): ByteArray = device.storage.requireDatabase()
    .rawQuery("SELECT record FROM signal_sessions WHERE peer_void_id=? AND device_id=1", arrayOf(peer.voidId)).use {
      check(it.moveToFirst())
      it.getBlob(0)
    }

  private fun claim(device: Device): JSONObject = device.messaging().claimReceiptUpload().getJSONObject("operation")

  private fun finishReceipt(sender: Device, operation: JSONObject) {
    val id = operation.getString("id")
    sender.messaging().completeReceiptUpload(id, frame("accepted", id))
  }

  @Test fun realReceiptsSurviveReopenWithoutOriginalAckOrFirstAcceptance() = withPeers { alice, bob ->
    val conversation = establish(alice, bob)
    val old = queue(alice, conversation, "Old message without opt-in")
    val oldId = old.getString("id")
    alice.messaging().completeOperation(oldId, frame("accepted", oldId))
    assertFalse(apply(bob, old, alice).optBoolean("receiptQueued"))
    assertTrue(bob.messaging().claimReceiptUpload().isNull("operation"))

    // Establish both sending chains before exercising the opted-in application path.
    val bobConversation = value(bob, "SELECT conversation_id FROM signal_threads WHERE peer_void_id=?", alice.voidId)!!
    val reply = queue(bob, bobConversation, "Legacy reply")
    val replyId = reply.getString("id")
    bob.messaging().completeOperation(replyId, frame("accepted", replyId))
    apply(alice, reply, bob)

    for (initialState in listOf("pending", "failed")) {
      val outgoing = queue(alice, conversation, "Opt-in from $initialState", request = true)
      val original = outgoing.getString("id")
      if (initialState == "failed") alice.messaging().failOperation(original, "unavailable")
      assertEquals(initialState, state(alice, original))
      assertTrue(apply(bob, outgoing, alice).getBoolean("receiptQueued"))
      assertFalse(apply(bob, outgoing, alice).getBoolean("changed"))
      assertEquals("1", value(bob, "SELECT count(*) FROM signal_control_outbox WHERE original_message_id=?", original))

      // The store is reopened before receipt preparation with no original ack_ok.
      bob.reopen()
      val receipt = claim(bob)
      val receiptId = receipt.getString("id")
      assertNotEquals(original, receiptId)
      val payload = JSONObject(MessagingProtocol.bytes(receipt.getString("payloadBase64")).toString(Charsets.UTF_8))
      assertEquals(receiptId, payload.getString("client_message_id"))
      assertFalse(payload.has("original_message_id"))

      // The first transport result is lost. Make the saved retry eligible without waiting.
      bob.reopen()
      bob.storage.requireDatabase().execSQL("UPDATE signal_control_outbox SET next_attempt_at=0 WHERE event_id=?", arrayOf(receiptId))
      val retry = claim(bob)
      assertEquals(receiptId, retry.getString("id"))
      assertTrue("A retry must reuse the committed ciphertext", receipt.getString("payloadBase64") == retry.getString("payloadBase64"))
      assertTrue(apply(alice, retry, bob).getBoolean("changed"))
      assertEquals("delivered", state(alice, original))
      assertEquals(original, value(alice, "SELECT original_message_id FROM signal_control_inbox WHERE sender_void_id=? AND event_id=? AND disposition='applied'", bob.voidId, receiptId))
      assertTrue(message(alice, original).getLong("verifiedDeliveredAt") > 0L)
      assertNull(state(alice, receiptId))
      assertTrue(alice.messaging().claimReceiptUpload().isNull("operation"))
      assertFalse(apply(alice, retry, bob).getBoolean("changed"))
      finishReceipt(bob, retry)
      assertEquals("accepted", value(bob, "SELECT status FROM signal_control_outbox WHERE event_id=?", receiptId))
      assertNull(value(bob, "SELECT payload_base64 FROM signal_control_outbox WHERE event_id=?", receiptId))

      alice.messaging().completeOperation(original, frame("accepted", original))
      alice.messaging().failOperation(original, "unavailable")
      alice.messaging().retryOperation(original)
      assertEquals("delivered", state(alice, original))
      alice.storage.execute("messages.setDeliveryState", JSONObject().put("id", original).put("state", "read"))
      assertFalse(apply(alice, retry, bob).getBoolean("changed"))
      assertEquals("read", state(alice, original))
    }
    alice.reopen()
    assertEquals("sent", state(alice, oldId))
    alice.storage.execute("messages.setDeliveryState", JSONObject().put("id", oldId).put("state", "delivered"))
    assertTrue("A local state label is not a verified receipt", message(alice, oldId).isNull("verifiedDeliveredAt"))
    assertEquals("Old message without opt-in", value(bob, "SELECT body FROM messages WHERE id=?", oldId))
    assertEquals("4", value(alice, "SELECT count(*) FROM messages WHERE conversation_id=?", conversation))
    assertEquals("4", value(bob, "SELECT count(*) FROM messages WHERE conversation_id=?", bobConversation))
  }

  @Test fun originalMessageAndReceiptObligationRollBackTogether() = withPeers { alice, bob ->
    val conversation = establish(alice, bob)
    val outgoing = queue(alice, conversation, "Must commit atomically", request = true)
    val original = outgoing.getString("id")
    bob.storage.requireDatabase().execSQL("CREATE TRIGGER test_fail_receipt BEFORE INSERT ON signal_control_outbox BEGIN SELECT RAISE(ABORT, 'test_rollback'); END;")
    assertTrue(runCatching { bob.messaging().applyEvent(event(outgoing, alice), alice.person) }.isFailure)
    assertNull(state(bob, original))
    assertEquals("0", value(bob, "SELECT count(*) FROM signal_inbox"))
    assertEquals("0", value(bob, "SELECT count(*) FROM signal_control_outbox"))
    bob.storage.requireDatabase().execSQL("DROP TRIGGER test_fail_receipt")
    bob.reopen()
    assertTrue(apply(bob, outgoing, alice).getBoolean("receiptQueued"))
    assertEquals("received", state(bob, original))
    val before = ratchet(bob, alice)
    val sql = "CREATE TRIGGER test_fail_ciphertext BEFORE UPDATE OF payload_base64 ON signal_control_outbox WHEN NEW.status='pending' BEGIN SELECT RAISE(ABORT, 'test_rollback'); END;"
    try {
      bob.storage.requireDatabase().execSQL(sql)
      val claim = runCatching { bob.messaging().claimReceiptUpload() }
      assertTrue(claim.isFailure || claim.getOrThrow().isNull("operation"))
      assertEquals("ready", value(bob, "SELECT status FROM signal_control_outbox WHERE original_message_id=?", original))
      assertNull(value(bob, "SELECT payload_base64 FROM signal_control_outbox WHERE original_message_id=?", original))
      val after = ratchet(bob, alice)
      try { assertTrue("The failed ciphertext commit must roll back its ratchet update", before.contentEquals(after)) }
      finally { after.fill(0) }
    } finally { before.fill(0) }
    bob.storage.requireDatabase().execSQL("DROP TRIGGER test_fail_ciphertext")
    bob.reopen()
    bob.storage.requireDatabase().execSQL("UPDATE signal_control_outbox SET next_attempt_at=0 WHERE original_message_id=?", arrayOf(original))
    assertTrue(apply(alice, claim(bob), bob).getBoolean("changed"))
    assertEquals("delivered", state(alice, original))
  }

  /** Build adversarial but authentic controls without adding a production fixture hook. */
  private fun signedControl(sender: Device, recipient: Device, original: String, innerRecipient: String = recipient.voidId): JSONObject =
    sender.storage.transaction {
      val db = sender.storage.requireDatabase()
      val store = SignalStore(db)
      val eventId = uuid()
      val content = JSONObject().put("v", 1).put("type", "delivery_receipt").put("client_message_id", eventId)
        .put("from_void_id", sender.voidId).put("to_void_id", innerRecipient).put("original_message_id", original)
        .toString().toByteArray(Charsets.UTF_8)
      val cipher = SessionCipher(store, store, store, store, store,
        SignalProtocolAddress(sender.voidId, 1), SignalProtocolAddress(recipient.voidId, 1))
      val ciphertext = try { cipher.encrypt(content) } finally { content.fill(0) }
      val identity = MessagingProtocol.base64(store.getIdentityKeyPair().publicKey.serialize())
      val registration = store.getLocalRegistrationId()
      val binding = "VOID:signal-identity:v1\n$audience\n${sender.voidId}\n1\n$registration\n$identity\n"
      val signature = MessagingProtocol.base64(AccountKeyStore(db).sign(binding.toByteArray(Charsets.UTF_8)))
      val payload = JSONObject().put("v", 2).put("kind", "signal").put("client_message_id", eventId)
        .put("from_void_id", sender.voidId).put("to_void_id", recipient.voidId).put("device_id", 1)
        .put("registration_id", registration).put("identity_key", identity).put("account_signature", signature)
        .put("message_type", ciphertext.type).put("ciphertext_base64", MessagingProtocol.base64(ciphertext.serialize()))
      JSONObject().put("id", eventId).put("payloadBase64", MessagingProtocol.base64(payload.toString().toByteArray(Charsets.UTF_8)))
    }

  @Test fun controlsMustMatchThePinnedPeerAndOriginalAndCannotOverwriteReplays() = withPeers { alice, bob ->
    val conversation = establish(alice, bob)
    val outgoing = queue(alice, conversation, "Only Bob can deliver this", request = true)
    val original = outgoing.getString("id")
    apply(bob, outgoing, alice)
    val charlie = Device("110559210000000013")
    try {
      val otherConversation = establish(alice, charlie)
      apply(charlie, queue(alice, otherConversation, "Establish a different verified peer"), alice)
      val wrongPeer = signedControl(charlie, alice, original)
      assertFalse(apply(alice, wrongPeer, charlie).getBoolean("changed"))
      assertEquals("ignored", value(alice, "SELECT disposition FROM signal_control_inbox WHERE event_id=?", wrongPeer.getString("id")))
      assertEquals("pending", state(alice, original))

      val wrongRecipient = signedControl(bob, alice, original, innerRecipient = charlie.voidId)
      assertEquals("SIGNAL_INVALID_ENVELOPE", (runCatching { apply(alice, wrongRecipient, bob) }.exceptionOrNull() as? SignalFailure)?.code)
      assertNull(value(alice, "SELECT disposition FROM signal_control_inbox WHERE event_id=?", wrongRecipient.getString("id")))

      val unknown = signedControl(bob, alice, uuid())
      assertFalse(apply(alice, unknown, bob).getBoolean("changed"))
      assertFalse(apply(alice, unknown, bob).getBoolean("changed"))
      assertEquals("ignored", value(alice, "SELECT disposition FROM signal_control_inbox WHERE event_id=?", unknown.getString("id")))
      assertEquals("pending", state(alice, original))

      val correct = claim(bob)
      val unsigned = JSONObject(MessagingProtocol.bytes(correct.getString("payloadBase64")).toString(Charsets.UTF_8))
        .put("account_signature", MessagingProtocol.base64(ByteArray(64)))
      val invalidBinding = event(correct, bob).put("payload_base64", MessagingProtocol.base64(unsigned.toString().toByteArray(Charsets.UTF_8)))
      assertEquals("SIGNAL_ACCOUNT_BINDING_FAILED", (runCatching { alice.messaging().applyEvent(invalidBinding, bob.person) }.exceptionOrNull() as? SignalFailure)?.code)
      val spoofed = event(correct, bob).put("from_void_id", charlie.voidId)
      assertEquals("SIGNAL_INVALID_ENVELOPE", (runCatching { alice.messaging().applyEvent(spoofed, charlie.person) }.exceptionOrNull() as? SignalFailure)?.code)
      assertTrue(apply(alice, correct, bob).getBoolean("changed"))
      val conflict = event(correct, bob).put("payload_base64", unknown.getString("payloadBase64"))
      assertEquals("SIGNAL_EVENT_ID_CONFLICT", (runCatching { alice.messaging().applyEvent(conflict, bob.person) }.exceptionOrNull() as? SignalFailure)?.code)
      assertEquals("delivered", state(alice, original))
      assertTrue(alice.messaging().claimReceiptUpload().isNull("operation"))
    } finally { charlie.close() }
  }
}
