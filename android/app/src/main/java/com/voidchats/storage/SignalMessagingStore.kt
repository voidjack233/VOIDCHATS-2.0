package com.voidchats.storage

import android.util.JsonReader
import android.util.JsonToken
import java.io.IOException
import java.io.StringReader
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject
import org.signal.libsignal.protocol.IdentityKey
import org.signal.libsignal.protocol.SessionBuilder
import org.signal.libsignal.protocol.SessionCipher
import org.signal.libsignal.protocol.SignalProtocolAddress
import org.signal.libsignal.protocol.ecc.ECPublicKey
import org.signal.libsignal.protocol.kem.KEMPublicKey
import org.signal.libsignal.protocol.message.CiphertextMessage
import org.signal.libsignal.protocol.message.PreKeySignalMessage
import org.signal.libsignal.protocol.message.SignalMessage
import org.signal.libsignal.protocol.state.PreKeyBundle

internal class SignalFailure(val code: String) : RuntimeException("The protected Signal operation failed.")

/** Signal callbacks, history and transport bookkeeping share a SQLCipher commit.
 * A fresh SignalStore loads every record; a rolled-back operation leaves no
 * cached ratchet that could run ahead of durable state. */
internal class SignalMessagingStore(
  private val storage: AppStorage,
  private val requestDeliveryReceipts: Boolean = false
) {
  private companion object {
    const val DAY_MS = 86_400_000L
    const val RECEIPT_DEADLINE_MS = 30 * DAY_MS
    const val RECEIPT_TERMINAL_RETENTION_MS = 7 * DAY_MS
    const val RECEIPT_DEDUP_RETENTION_MS = 61 * DAY_MS
    const val RECEIPT_MAX_ATTEMPTS = 8
    const val RECEIPT_INITIAL_DELAY_MS = 30_000L
    const val RECEIPT_MAX_DELAY_MS = 3_600_000L
  }

  private enum class VerificationStage { BINDING, BUNDLE_PROCESS, ENCRYPT, PREKEY_PARSE, PREKEY_DECRYPT, MESSAGE_PARSE, MESSAGE_DECRYPT }
  private val db get() = storage.requireDatabase()
  private val manualRetryErrors = setOf(
    "unavailable", "deadline_exceeded", "queue_full", "rate_limited", "overloaded",
    "unauthorized", "peer_offline", "prekeys_unavailable", "prekeys_capacity", "send_interrupted"
  )
  private fun identity() = (storage.localIdentity() ?: throw StorageFailure("NOT_INITIALIZED")).also {
    if (it.isNull("accountVoidId") || it.isNull("accountApiUrl")) throw StorageFailure("NOT_INITIALIZED")
  }
  private fun ownId() = identity().getString("accountVoidId")
  private fun id() = UUID.randomUUID().toString()
  private fun now() = System.currentTimeMillis()
  private inline fun diagnosticAfterCommit(operation: () -> Unit) {
    try {
      // Nested fixture or caller transactions have not durably committed yet.
      if (!db.inTransaction()) operation()
    } catch (_: Throwable) { /* Diagnostic reads and logging cannot change processing results. */ }
  }
  private fun rows(sql: String, vararg args: String): JSONArray = db.rawQuery(sql, args).use { cursor ->
    JSONArray().apply {
      while (cursor.moveToNext()) put(JSONObject().apply {
        for (column in 0 until cursor.columnCount) put(cursor.getColumnName(column), when {
          cursor.isNull(column) -> JSONObject.NULL
          cursor.getType(column) == android.database.Cursor.FIELD_TYPE_INTEGER -> cursor.getLong(column)
          else -> cursor.getString(column)
        })
      })
    }
  }
  private fun one(sql: String, vararg args: String): JSONObject? = rows(sql, *args).let { if (it.length() == 0) null else it.getJSONObject(0) }
  private fun address(peer: String) = SignalProtocolAddress(MessagingProtocol.voidId(peer), 1)
  private fun peer(row: JSONObject) = JSONObject().put("void_id", row.getString("peer_void_id")).put("auth_public_key", row.getString("account_key_base64"))
  private fun available(peer: String) {
    if (one("SELECT peer_void_id FROM messaging_blocks WHERE peer_void_id=?", peer) != null) throw SignalFailure("SIGNAL_PEER_BLOCKED")
  }
  private fun thread(conversationId: String): JSONObject = one("SELECT * FROM signal_threads WHERE conversation_id=?", MessagingProtocol.uuid(conversationId))
    ?: throw SignalFailure("SIGNAL_THREAD_MISSING")
  private fun clearSendFailure(row: JSONObject) {
    check(db.inTransaction())
    if (row.isNull("last_error")) return
    if (row.getString("last_error").lowercase() !in manualRetryErrors) throw SignalFailure("SIGNAL_SEND_PAUSED")
    db.execSQL("UPDATE signal_threads SET last_error=NULL WHERE conversation_id=?", arrayOf(row.getString("conversation_id")))
  }
  private fun failSavedMessage(messageId: String, errorCode: String) {
    check(db.inTransaction())
    val message = one("SELECT * FROM messages WHERE id=?", messageId) ?: throw SignalFailure("SIGNAL_OPERATION_MISSING")
    require(message.getString("direction") == "outgoing" && message.getString("sender_void_id") == ownId())
    val outbox = one("SELECT status FROM signal_outbox WHERE event_id=?", messageId)
    if (message.getString("delivery_state") !in setOf("pending", "failed") || outbox?.getString("status") == "done") return
    val draft = one("SELECT conversation_id FROM signal_drafts WHERE message_id=?", messageId)
    if (outbox == null && draft == null) throw SignalFailure("SIGNAL_OPERATION_MISSING")
    db.execSQL("UPDATE signal_outbox SET status='failed',error_code=? WHERE event_id=? AND status<>'done'", arrayOf(errorCode, messageId))
    db.execSQL("UPDATE messages SET delivery_state='failed' WHERE id=? AND delivery_state='pending'", arrayOf(messageId))
    if (draft != null) {
      val row = thread(draft.getString("conversation_id"))
      // Transport failure must not replace an identity or cryptographic pause.
      if (row.isNull("last_error") || row.getString("last_error").lowercase() in manualRetryErrors) {
        db.execSQL("UPDATE signal_threads SET last_error=? WHERE conversation_id=?", arrayOf(errorCode.uppercase(), row.getString("conversation_id")))
      }
    }
  }
  private fun storedRecordFailure(error: Exception): String? {
    // Class/method metadata only. Preserve the original store exception and
    // transaction behavior; never inspect or report record bytes or messages.
    if (error.stackTrace.none { it.className == SignalStore::class.java.name && it.methodName == "decoded" }) return null
    // An identity-pair constructor can call an identity-key constructor; report
    // the outer stored record rather than its nested public-key object.
    return error.stackTrace.reversed().firstNotNullOfOrNull { frame ->
      if (frame.methodName != "<init>") null else when (frame.className) {
        "org.signal.libsignal.protocol.IdentityKeyPair" -> "SIGNAL_LOCAL_IDENTITY_RECORD_INVALID"
        "org.signal.libsignal.protocol.IdentityKey" -> "SIGNAL_PEER_IDENTITY_RECORD_INVALID"
        "org.signal.libsignal.protocol.state.SessionRecord" -> "SIGNAL_SESSION_RECORD_INVALID"
        "org.signal.libsignal.protocol.state.PreKeyRecord" -> "SIGNAL_EC_PREKEY_RECORD_INVALID"
        "org.signal.libsignal.protocol.state.SignedPreKeyRecord" -> "SIGNAL_SIGNED_PREKEY_RECORD_INVALID"
        "org.signal.libsignal.protocol.state.KyberPreKeyRecord" -> "SIGNAL_KYBER_PREKEY_RECORD_INVALID"
        else -> null
      }
    }
  }
  private fun <T> signal(stage: VerificationStage? = null, operation: () -> T): T = try { operation() }
  catch (error: SignalFailure) { throw error }
  catch (error: StorageFailure) { throw error }
  catch (error: android.database.SQLException) { throw StorageFailure("DATABASE_ERROR") }
  catch (error: Exception) {
    // No raw exception text or cryptographic objects cross the native bridge.
    val scope = stage?.let { "${it.name}_" } ?: ""
    val code = when (error.javaClass.simpleName) {
      "UntrustedIdentityException" -> "SIGNAL_IDENTITY_CHANGED"
      "InvalidKeyIdException" -> "SIGNAL_PREKEY_UNAVAILABLE"
      "NoSessionException" -> "SIGNAL_SESSION_UNAVAILABLE"
      "DuplicateMessageException" -> "SIGNAL_DUPLICATE_MESSAGE"
      "ReusedBaseKeyException" -> "SIGNAL_REUSED_BASE_KEY"
      "InvalidMessageException" -> storedRecordFailure(error) ?: "SIGNAL_${scope}INVALID_MESSAGE"
      "InvalidKeyException" -> storedRecordFailure(error) ?: "SIGNAL_${scope}INVALID_KEY"
      "InvalidVersionException" -> storedRecordFailure(error) ?: "SIGNAL_${scope}INVALID_VERSION"
      "LegacyMessageException" -> storedRecordFailure(error) ?: "SIGNAL_${scope}LEGACY_MESSAGE"
      "IllegalArgumentException", "JSONException" -> "SIGNAL_INVALID_ENVELOPE"
      else -> "SIGNAL_ENGINE_FAILURE"
    }
    throw SignalFailure(code)
  }

  fun signAuthChallenge(messageBase64: String): String = MessagingProtocol.base64(
    AccountKeyStore(db).sign(MessagingProtocol.authMessage(messageBase64, identity())))

  private fun verifiedThread(person: JSONObject): JSONObject? {
    check(db.inTransaction())
    MessagingProtocol.person(person)
    val peerId = person.getString("void_id")
    require(peerId != ownId()); available(peerId)
    val accountKey = person.getString("auth_public_key")
    val contact = one("SELECT identity_public_key FROM contacts WHERE void_id=?", peerId)
    if (contact != null && contact.getString("identity_public_key") != accountKey) throw SignalFailure("SIGNAL_IDENTITY_CHANGED")
    val old = one("SELECT * FROM signal_threads WHERE peer_void_id=?", peerId)
    if (old != null && old.getString("account_key_base64") != accountKey) throw SignalFailure("SIGNAL_IDENTITY_CHANGED")
    return old
  }

  private fun ensureThread(person: JSONObject, incoming: Boolean): JSONObject =
    verifiedThread(person) ?: createThread(person, incoming)

  private fun createThread(person: JSONObject, incoming: Boolean): JSONObject {
    val peerId = person.getString("void_id")
    val accountKey = person.getString("auth_public_key")
    val existing = one("SELECT id FROM conversations WHERE kind='direct' AND peer_void_id=?", peerId)
    val conversationId = existing?.getString("id") ?: id()
    if (existing == null) PublicRepositories(db).execute("conversations.save", JSONObject()
      .put("id", conversationId).put("kind", "direct").put("title", peerId).put("peerVoidId", peerId)
      .put("mlsGroupId", JSONObject.NULL).put("createdAt", now()).put("updatedAt", now()))
    db.execSQL("INSERT INTO signal_threads(conversation_id,peer_void_id,account_key_base64,direction,is_request,claim_id) VALUES(?,?,?,?,?,?)",
      arrayOf(conversationId, peerId, accountKey, if (incoming) "incoming" else "outgoing", if (incoming) 1 else 0, id()))
    return thread(conversationId)
  }

  fun openConversation(person: JSONObject): JSONObject {
    val row = storage.transaction { ensureThread(person, false) }
    return PublicRepositories(db).execute("conversations.get", JSONObject().put("id", row.getString("conversation_id"))) as JSONObject
  }

  fun state(): JSONObject {
    identity()
    val store = SignalStore(db)
    val requests = JSONArray()
    val sessions = rows("SELECT * FROM signal_threads ORDER BY conversation_id").let { data -> JSONArray().apply {
      for (index in 0 until data.length()) {
        val row = data.getJSONObject(index); val conversationId = row.getString("conversation_id")
        val peerId = row.getString("peer_void_id")
        val blocked = one("SELECT peer_void_id FROM messaging_blocks WHERE peer_void_id=?", peerId) != null
        val established = store.containsSession(address(peerId))
        val hasDraft = one("SELECT d.message_id FROM signal_drafts d JOIN messages m ON m.id=d.message_id WHERE d.conversation_id=? AND m.delivery_state='pending' LIMIT 1", conversationId) != null
        if (!blocked && !established && hasDraft && row.isNull("last_error")) requests.put(
          JSONObject().put("id", row.getString("claim_id")).put("peer", peer(row)))
        put(JSONObject().put("id", conversationId).put("peer", peer(row)).put("direction", row.getString("direction"))
          .put("status", if (blocked) "blocked" else if (established) "active" else "pending")
          .put("isRequest", row.getInt("is_request") == 1)
          .put("hasMessages", one("SELECT id FROM messages WHERE conversation_id=? LIMIT 1", conversationId) != null)
          .put("diagnosticCode", if (row.isNull("last_error")) JSONObject.NULL else row.getString("last_error")))
      }
    } }
    val operations = rows("SELECT * FROM signal_outbox o WHERE status<>'done' AND NOT EXISTS (SELECT 1 FROM messaging_blocks b WHERE b.peer_void_id=o.peer_void_id) ORDER BY created_at,event_id LIMIT 256").let { data -> JSONArray().apply {
      for (index in 0 until data.length()) {
        val row = data.getJSONObject(index)
        put(JSONObject().put("id", row.getString("event_id")).put("kind", "message").put("purpose", "application")
          .put("peerVoidId", row.getString("peer_void_id")).put("payloadBase64", row.getString("payload_base64"))
          .put("status", row.getString("status")).put("errorCode", row.opt("error_code") ?: JSONObject.NULL))
      }
    } }
    return JSONObject().put("sessions", sessions).put("outbox", operations).put("prekeyRequests", requests)
      .put("blockedVoidIds", rows("SELECT peer_void_id FROM messaging_blocks").let { data -> JSONArray().apply {
        for (index in 0 until data.length()) put(data.getJSONObject(index).getString("peer_void_id"))
      } })
  }

  private fun bindingMessage(owner: String, device: Int, registration: Int, key: String): ByteArray {
    MessagingProtocol.voidId(owner); require(device == 1 && registration in 1..16_380)
    MessagingProtocol.bytes(key, 33).also { require(it.size == 33) }
    val audience = identity().getString("accountApiUrl")
    RegistrationProof.validateApiUrl(audience)
    return "VOID:signal-identity:v1\n$audience\n$owner\n$device\n$registration\n$key\n".toByteArray(Charsets.UTF_8)
  }
  private fun ownBinding(store: SignalStore): JSONObject {
    val key = MessagingProtocol.base64(store.getIdentityKeyPair().publicKey.serialize())
    val registration = store.getLocalRegistrationId()
    val signature = AccountKeyStore(db).sign(bindingMessage(ownId(), 1, registration, key))
    return JSONObject().put("device_id", 1).put("registration_id", registration).put("identity_key", key)
      .put("account_signature", MessagingProtocol.base64(signature))
  }

  private fun encodeSignalPayload(store: SignalStore, eventId: String, peerId: String, encrypted: CiphertextMessage): String {
    val payload = ownBinding(store).put("v", 2).put("kind", "signal").put("client_message_id", eventId)
      .put("from_void_id", ownId()).put("to_void_id", peerId).put("message_type", encrypted.type)
      .put("ciphertext_base64", MessagingProtocol.base64(encrypted.serialize()))
    return MessagingProtocol.base64(payload.toString().toByteArray(Charsets.UTF_8))
  }
  private fun verifyBinding(binding: JSONObject, person: JSONObject, store: SignalStore): IdentityKey {
    MessagingProtocol.person(person)
    val owner = person.getString("void_id")
    val key = MessagingProtocol.text(binding, "identity_key", 44)
    val message = bindingMessage(owner, binding.getInt("device_id"), binding.getInt("registration_id"), key)
    val accountKey = MessagingProtocol.bytes(person.getString("auth_public_key"), 32)
    val signature = MessagingProtocol.bytes(MessagingProtocol.text(binding, "account_signature", 88), 64)
    if (!AccountKeyStore.verify(accountKey, message, signature)) throw SignalFailure("SIGNAL_ACCOUNT_BINDING_FAILED")
    return IdentityKey(MessagingProtocol.bytes(key, 33)).also { store.bindIdentity(owner, it) }
  }

  fun getPreKeyPublication(): Any = signal { storage.transaction {
    identity()
    val old = one("SELECT * FROM signal_publication WHERE slot=1")
    if (old != null) {
      if (old.getInt("published") == 1) return@transaction JSONObject.NULL
      return@transaction JSONObject().put("id", old.getString("publication_id")).put("payloadBase64", old.getString("payload_base64"))
    }
    val store = SignalStore(db)
    val publication = store.prekeyPublication()
    publication.put("v", 1)
    val binding = ownBinding(store)
    publication.put("account_signature", binding.getString("account_signature"))
    val encoded = MessagingProtocol.base64(publication.toString().toByteArray(Charsets.UTF_8)); val publicationId = id()
    db.execSQL("INSERT INTO signal_publication(slot,publication_id,payload_base64) VALUES(1,?,?)", arrayOf(publicationId, encoded))
    JSONObject().put("id", publicationId).put("payloadBase64", encoded)
  } }

  fun completePreKeyPublication(publicationId: String): JSONObject = signal { storage.transaction {
    val old = one("SELECT * FROM signal_publication WHERE slot=1 AND publication_id=?", MessagingProtocol.uuid(publicationId))
      ?: throw SignalFailure("SIGNAL_PUBLICATION_MISMATCH")
    val publication = JSONObject(MessagingProtocol.bytes(old.getString("payload_base64")).toString(Charsets.UTF_8))
    SignalStore(db).markPublished(publication)
    db.execSQL("UPDATE signal_publication SET published=1 WHERE slot=1")
    JSONObject()
  } }

  fun establishSignalSession(person: JSONObject, encoded: String): JSONObject {
    var created = false
    signal { storage.transaction {
      val row = ensureThread(person, false); val store = SignalStore(db); val peerId = row.getString("peer_void_id")
      val bundle = JSONObject(MessagingProtocol.bytes(encoded).toString(Charsets.UTF_8))
      require(bundle.getInt("v") == 1)
      val expires = RegistrationProof.timestampMillis(MessagingProtocol.text(bundle, "claim_expires_at", 40))
      if (expires <= now()) throw SignalFailure("SIGNAL_PREKEY_CLAIM_EXPIRED")
      val key = verifyBinding(bundle, person, store)
      if (!store.containsSession(address(peerId))) {
        val signed = bundle.getJSONObject("signed_prekey")
        val kyber = bundle.getJSONObject("kyber_prekey")
        val oneTime = bundle.optJSONObject("one_time_prekey")
        fun keyId(record: JSONObject): Int = record.getInt("key_id").also { require(it in 1..16_777_214) }
        fun publicKey(record: JSONObject) = MessagingProtocol.bytes(MessagingProtocol.text(record, "public_key", 4096), 3072)
        fun signature(record: JSONObject) = MessagingProtocol.bytes(MessagingProtocol.text(record, "signature", 88), 64).also { require(it.size == 64) }
        val prekey = PreKeyBundle(bundle.getInt("registration_id"), 1,
          oneTime?.let(::keyId) ?: PreKeyBundle.NULL_PRE_KEY_ID, oneTime?.let { ECPublicKey(publicKey(it)) },
          keyId(signed), ECPublicKey(publicKey(signed)), signature(signed), key,
          keyId(kyber), KEMPublicKey(publicKey(kyber)), signature(kyber))
        signal(VerificationStage.BUNDLE_PROCESS) {
          SessionBuilder(store, store, store, store, address(peerId), address(ownId())).process(prekey)
        }
        created = true
      }
      db.execSQL("UPDATE signal_threads SET last_error=NULL WHERE conversation_id=?", arrayOf(row.getString("conversation_id")))
    } }
    diagnosticAfterCommit {
      val peerId = person.getString("void_id")
      val record = SignalStore(db).loadSession(address(peerId))
      SignalDecryptDiagnostics.logSession(ownId(), peerId, created, record.hasSenderChain(), record.sessionVersion)
    }
    return resumePending()
  }

  fun queueMessage(conversationId: String, body: String): JSONObject {
    require(body.isNotBlank() && !body.contains('\u0000') && body.toByteArray(Charsets.UTF_8).size <= 60_000)
    storage.transaction {
      // Adopt only public metadata when the user sends in an existing thread.
      // Legacy MLS keys, checkpoints and encrypted outbox entries stay dormant.
      if (one("SELECT conversation_id FROM signal_threads WHERE conversation_id=?", MessagingProtocol.uuid(conversationId)) == null) {
        val legacy = one("SELECT s.peer_void_id,s.peer_key_base64 FROM messaging_sessions s JOIN conversations c ON c.peer_void_id=s.peer_void_id WHERE c.id=? AND c.kind='direct' ORDER BY s.created_at DESC LIMIT 1", conversationId)
          ?: throw SignalFailure("SIGNAL_THREAD_MISSING")
        ensureThread(JSONObject().put("void_id", legacy.getString("peer_void_id")).put("auth_public_key", legacy.getString("peer_key_base64")), false)
      }
      val row = thread(conversationId); available(row.getString("peer_void_id"))
      clearSendFailure(row)
      val messageId = id()
      PublicRepositories(db).execute("messages.save", JSONObject().put("id", messageId).put("conversationId", conversationId)
        .put("senderVoidId", ownId()).put("body", body).put("direction", "outgoing").put("deliveryState", "pending").put("createdAt", now()))
      db.execSQL("INSERT INTO signal_drafts(message_id,conversation_id) VALUES(?,?)", arrayOf(messageId, conversationId))
      db.execSQL("UPDATE signal_threads SET is_request=0 WHERE conversation_id=?", arrayOf(conversationId))
    }
    return resumePending()
  }

  private fun flushDraft(row: JSONObject, draft: JSONObject) = signal {
    var committedEventId: String? = null
    var committedPayload: String? = null
    var draftCreatedAt = 0L
    var encryptedAt = 0L
    storage.transaction {
      val current = one("SELECT m.* FROM signal_drafts d JOIN messages m ON m.id=d.message_id WHERE d.message_id=? AND m.delivery_state='pending'", draft.getString("id"))
        ?: return@transaction
      val peerId = row.getString("peer_void_id"); available(peerId)
      if (!thread(row.getString("conversation_id")).isNull("last_error")) return@transaction
      val store = SignalStore(db)
      if (!store.containsSession(address(peerId))) return@transaction
      val messageId = current.getString("id")
      val message = JSONObject().put("v", 2).put("client_message_id", messageId).put("from_void_id", ownId())
        .put("to_void_id", peerId).put("body", current.getString("body")).put("created_at", current.getLong("created_at"))
      if (requestDeliveryReceipts) message.put("request_delivery_receipt", 1)
      val content = message.toString().toByteArray(Charsets.UTF_8)
      try {
        val encrypted = signal(VerificationStage.ENCRYPT) {
          SessionCipher(store, store, store, store, store, address(ownId()), address(peerId)).encrypt(content)
        }
        encryptedAt = now()
        val encoded = encodeSignalPayload(store, messageId, peerId, encrypted)
        db.execSQL("INSERT INTO signal_outbox(event_id,peer_void_id,payload_base64,status,created_at,receipt_requested) VALUES(?,?,?,'pending',?,?)",
          arrayOf(messageId, peerId, encoded, current.getLong("created_at"), if (requestDeliveryReceipts) 1 else 0))
        db.execSQL("DELETE FROM signal_drafts WHERE message_id=?", arrayOf(messageId))
        committedEventId = messageId
        committedPayload = encoded
        draftCreatedAt = current.getLong("created_at")
      } finally { content.fill(0) }
    }
    diagnosticAfterCommit {
      val eventId = committedEventId
      val encoded = committedPayload
      if (eventId != null && encoded != null) {
        SignalDecryptDiagnostics.logCommittedEvent(SignalDecryptDiagnostics.CommitStage.ENCRYPT, ownId(), eventId, encoded,
          draftCreatedAt, encryptedAt)
      }
    }
  }

  fun resumePending(): JSONObject {
    identity()
    val drafts = rows("SELECT m.*,d.conversation_id FROM signal_drafts d JOIN messages m ON m.id=d.message_id WHERE m.delivery_state='pending' ORDER BY m.created_at,m.id")
    for (index in 0 until drafts.length()) {
      val draft = drafts.getJSONObject(index); val row = thread(draft.getString("conversation_id"))
      try { flushDraft(row, draft) }
      catch (error: SignalFailure) { storage.transaction {
        db.execSQL("UPDATE messages SET delivery_state='failed' WHERE id=? AND delivery_state='pending'", arrayOf(draft.getString("id")))
        db.execSQL("UPDATE signal_threads SET last_error=? WHERE conversation_id=?", arrayOf(error.code, row.getString("conversation_id")))
      } }
    }
    return state()
  }

  private fun saveReceiptObligation(sender: String, originalId: String) {
    check(db.inTransaction())
    val createdAt = now()
    db.execSQL("INSERT INTO signal_control_outbox(event_id,peer_void_id,original_message_id,created_at,next_attempt_at,deadline_at) VALUES(?,?,?,?,?,?)",
      arrayOf(id(), sender, originalId, createdAt, createdAt, createdAt + RECEIPT_DEADLINE_MS))
  }

  private fun pruneReceiptRecovery(at: Long) {
    db.execSQL("""
      UPDATE signal_control_outbox SET status='stopped',payload_base64=NULL,next_attempt_at=?
      WHERE status IN ('ready','pending') AND (deadline_at<=? OR (attempts>=? AND next_attempt_at<=?)
        OR EXISTS (SELECT 1 FROM messaging_blocks WHERE peer_void_id=signal_control_outbox.peer_void_id))
    """.trimIndent(), arrayOf(at, at, RECEIPT_MAX_ATTEMPTS, at))
    db.execSQL("DELETE FROM signal_control_outbox WHERE status IN ('accepted','stopped') AND next_attempt_at<=?",
      arrayOf(at - RECEIPT_TERMINAL_RETENTION_MS))
    db.execSQL("""
      DELETE FROM signal_control_inbox WHERE recorded_at<=? AND (disposition='ignored'
        OR NOT EXISTS (SELECT 1 FROM messages WHERE id=signal_control_inbox.original_message_id))
    """.trimIndent(), arrayOf(at - RECEIPT_DEDUP_RETENTION_MS))
  }

  private fun encryptReceipt(claim: JSONObject, sender: String): String? = storage.transaction {
    val eventId = claim.getString("event_id")
    val peerId = claim.getString("peer_void_id")
    val remote = address(peerId)
    val store = SignalStore(db)
    val pinned = store.getIdentity(remote) ?: return@transaction null
    val session = store.loadSession(remote)
    if (!session.hasSenderChain() || session.remoteIdentityKey != pinned) return@transaction null
    val content = JSONObject().put("v", 1).put("type", "delivery_receipt").put("client_message_id", eventId)
      .put("original_message_id", claim.getString("original_message_id")).put("from_void_id", sender)
      .put("to_void_id", peerId).toString().toByteArray(Charsets.UTF_8)
    try {
      val encrypted = signal(VerificationStage.ENCRYPT) {
        SessionCipher(store, store, store, store, store, address(sender), remote).encrypt(content)
      }
      val payload = encodeSignalPayload(store, eventId, peerId, encrypted)
      db.execSQL("UPDATE signal_control_outbox SET status='pending',payload_base64=? WHERE event_id=? AND status='ready'",
        arrayOf(payload, eventId))
      payload
    } finally { content.fill(0) }
  }

  fun claimReceiptUpload(): JSONObject = signal {
    val sender = ownId()
    val at = now()
    val claim = storage.transaction {
      pruneReceiptRecovery(at)
      val row = one("""
        SELECT event_id,peer_void_id,original_message_id,payload_base64,attempts FROM signal_control_outbox
        WHERE status IN ('ready','pending') AND attempts<? AND next_attempt_at<=? AND deadline_at>?
        ORDER BY next_attempt_at,created_at,event_id LIMIT 1
      """.trimIndent(), RECEIPT_MAX_ATTEMPTS.toString(), at.toString(), at.toString()) ?: return@transaction null
      val attempts = row.getInt("attempts")
      val delay = minOf(RECEIPT_MAX_DELAY_MS, RECEIPT_INITIAL_DELAY_MS shl attempts)
      // Reserve before encryption or network work so a crash or unavailable
      // session cannot leave this receipt immediately due on every restart.
      db.execSQL("UPDATE signal_control_outbox SET attempts=attempts+1,next_attempt_at=? WHERE event_id=?",
        arrayOf(at + delay, row.getString("event_id")))
      row
    }
    val operation = claim?.let { row ->
      val payload = if (!row.isNull("payload_base64")) row.getString("payload_base64") else {
        // The encryption transaction has rolled back if libsignal fails. The
        // retry reservation still stands and other obligations can proceed.
        try { signal { encryptReceipt(row, sender) } } catch (_: SignalFailure) { null }
      }
      payload?.let { JSONObject().put("id", row.getString("event_id")).put("peerVoidId", row.getString("peer_void_id")).put("payloadBase64", it) }
    }
    val next = one("""
      SELECT MIN(MIN(next_attempt_at,deadline_at)) AS next_at FROM signal_control_outbox WHERE status IN ('ready','pending')
    """.trimIndent())?.opt("next_at")
    JSONObject().put("operation", operation ?: JSONObject.NULL).put("nextAttemptAt", next ?: JSONObject.NULL)
  }

  fun completeReceiptUpload(operationId: String, response: Any): JSONObject {
    validateAcceptance(operationId, response)
    storage.transaction {
      db.execSQL("UPDATE signal_control_outbox SET status='accepted',payload_base64=NULL,next_attempt_at=? WHERE event_id=? AND status='pending'",
        arrayOf(now(), operationId))
    }
    return JSONObject()
  }

  fun stopReceiptUpload(operationId: String): JSONObject {
    storage.transaction {
      db.execSQL("UPDATE signal_control_outbox SET status='stopped',payload_base64=NULL,next_attempt_at=? WHERE event_id=? AND status IN ('ready','pending')",
        arrayOf(now(), MessagingProtocol.uuid(operationId)))
    }
    return JSONObject()
  }

  private fun receiptOriginalId(text: String, sender: String, recipient: String, eventId: String): String {
    // Read controls strictly so coercion, duplicate fields or trailing data
    // cannot change the meaning of an authenticated receipt.
    require(text.length <= 1024)
    val fields = mutableMapOf<String, String>()
    try {
      JsonReader(StringReader(text)).use { reader ->
        reader.beginObject()
        while (reader.hasNext()) {
          val key = reader.nextName()
          val token = if (key == "v") JsonToken.NUMBER else JsonToken.STRING
          require(key !in fields && reader.peek() == token)
          fields[key] = reader.nextString()
        }
        reader.endObject()
        require(reader.peek() == JsonToken.END_DOCUMENT)
      }
    } catch (_: IOException) { throw IllegalArgumentException() }
    require(fields.size == 6 && fields["v"] == "1" && fields["type"] == "delivery_receipt" &&
      fields["from_void_id"] == sender && fields["to_void_id"] == recipient && fields["client_message_id"] == eventId)
    return MessagingProtocol.uuid(requireNotNull(fields["original_message_id"])).also { require(it != eventId) }
  }

  private fun applyDeliveryReceipt(sender: String, recipient: String, eventId: String, originalId: String,
    digest: String, conversationId: String): Boolean {
    check(db.inTransaction())
    val eligible = one("""
      SELECT m.id FROM messages m JOIN signal_outbox o ON o.event_id=m.id
      WHERE m.id=? AND m.conversation_id=? AND m.direction='outgoing' AND m.sender_void_id=? AND m.deleted_at IS NULL
        AND o.peer_void_id=? AND o.receipt_requested=1
        AND NOT EXISTS (SELECT 1 FROM signal_control_inbox WHERE original_message_id=m.id AND disposition='applied')
    """.trimIndent(), originalId, conversationId, recipient, sender) != null
    if (eligible) {
      // The existing trigger requires pending/failed to advance through sent.
      db.execSQL("UPDATE messages SET delivery_state='sent' WHERE id=? AND delivery_state IN ('pending','failed')", arrayOf(originalId))
      db.execSQL("UPDATE messages SET delivery_state='delivered' WHERE id=? AND delivery_state='sent'", arrayOf(originalId))
      db.execSQL("UPDATE signal_outbox SET status='done',error_code=NULL WHERE event_id=?", arrayOf(originalId))
    }
    db.execSQL("INSERT INTO signal_control_inbox(sender_void_id,event_id,original_message_id,payload_hash,disposition,recorded_at) VALUES(?,?,?,?,?,?)",
      arrayOf(sender, eventId, originalId, digest, if (eligible) "applied" else "ignored", now()))
    return eligible
  }

  fun applyEvent(event: JSONObject, person: JSONObject): JSONObject = signal {
    val recipient = ownId(); require(event.optString("type") == "event")
    val sender = MessagingProtocol.voidId(MessagingProtocol.text(event, "from_void_id", 19))
    val eventId = MessagingProtocol.uuid(MessagingProtocol.text(event, "event_id", 36))
    require(person.getString("void_id") == sender && sender != recipient)
    val encoded = MessagingProtocol.text(event, "payload_base64", 1_398_104)
    val digest = MessagingProtocol.hash(encoded)
    val created = RegistrationProof.timestampMillis(MessagingProtocol.text(event, "created_at", 40))
    require(RegistrationProof.timestampMillis(MessagingProtocol.text(event, "expires_at", 40)) > created)
    // All failure paths roll back and throw; none returns an ACK token.
    var stage = SignalDecryptDiagnostics.CommitStage.RECEIVE
    val result = storage.transaction {
      val old = one("SELECT payload_hash FROM signal_inbox WHERE sender_void_id=? AND event_id=? " +
        "UNION ALL SELECT payload_hash FROM signal_control_inbox WHERE sender_void_id=? AND event_id=? LIMIT 1",
        sender, eventId, sender, eventId)
      if (old != null) {
        if (old.getString("payload_hash") != digest) throw SignalFailure("SIGNAL_EVENT_ID_CONFLICT")
        stage = SignalDecryptDiagnostics.CommitStage.DUPLICATE
        return@transaction JSONObject().put("sender_void_id", sender).put("event_id", eventId).put("changed", false)
      }
      if (one("SELECT id FROM messages WHERE id=?", eventId) != null) throw SignalFailure("SIGNAL_EVENT_ID_CONFLICT")
      val existingThread = verifiedThread(person)
      val payload = JSONObject(MessagingProtocol.bytes(encoded).toString(Charsets.UTF_8))
      require(payload.getInt("v") == 2 && payload.getString("kind") == "signal" &&
        payload.getString("from_void_id") == sender && payload.getString("to_void_id") == recipient &&
        payload.getString("client_message_id") == eventId)
      val store = SignalStore(db)
      val senderAddress = address(sender)
      val previousIdentity = existingThread?.let { store.getIdentity(senderAddress) }
      val boundKey = signal(VerificationStage.BINDING) { verifyBinding(payload, person, store) }
      val ciphertext = MessagingProtocol.bytes(MessagingProtocol.text(payload, "ciphertext_base64", 1_398_104))
      val cipher = SessionCipher(store, store, store, store, store, address(recipient), senderAddress)
      val plaintext = when (payload.getInt("message_type")) {
        CiphertextMessage.PREKEY_TYPE -> {
          val message = signal(VerificationStage.PREKEY_PARSE) { PreKeySignalMessage(ciphertext) }
          if (message.identityKey != boundKey || message.registrationId != payload.getInt("registration_id")) throw SignalFailure("SIGNAL_ACCOUNT_BINDING_FAILED")
          signal(VerificationStage.PREKEY_DECRYPT) {
            SignalDecryptDiagnostics.capture(SignalDecryptDiagnostics.DecryptStage.PREKEY, eventId, encoded) { cipher.decrypt(message) }
          }
        }
        CiphertextMessage.WHISPER_TYPE -> {
          val message = signal(VerificationStage.MESSAGE_PARSE) { SignalMessage(ciphertext) }
          signal(VerificationStage.MESSAGE_DECRYPT) {
            SignalDecryptDiagnostics.capture(SignalDecryptDiagnostics.DecryptStage.MESSAGE, eventId, encoded) { cipher.decrypt(message) }
          }
        }
        else -> throw SignalFailure("SIGNAL_UNSUPPORTED_MESSAGE")
      }
      try {
        val session = store.loadSession(senderAddress)
        if (session.remoteIdentityKey != boundKey) throw SignalFailure("SIGNAL_ACCOUNT_BINDING_FAILED")
        val text = plaintext.toString(Charsets.UTF_8)
        val content = JSONObject(text)
        if (content.has("type")) {
          if (content.opt("type") != "delivery_receipt") throw SignalFailure("SIGNAL_UNSUPPORTED_MESSAGE")
          if (existingThread == null || previousIdentity != boundKey) throw SignalFailure("SIGNAL_ACCOUNT_BINDING_FAILED")
          val originalId = receiptOriginalId(text, sender, recipient, eventId)
          val changed = applyDeliveryReceipt(sender, recipient, eventId, originalId, digest, existingThread.getString("conversation_id"))
          stage = SignalDecryptDiagnostics.CommitStage.RECEIPT
          return@transaction JSONObject().put("sender_void_id", sender).put("event_id", eventId).put("changed", changed)
        }
        val at = content.getLong("created_at")
        require(content.getInt("v") == 2 && content.getString("client_message_id") == eventId &&
          content.getString("from_void_id") == sender && content.getString("to_void_id") == recipient && at in 0..9_007_199_254_740_991L)
        val body = MessagingProtocol.text(content, "body", 60_000)
        require(body.isNotBlank() && body.toByteArray(Charsets.UTF_8).size <= 60_000)
        val row = existingThread ?: createThread(person, true)
        PublicRepositories(db).execute("messages.save", JSONObject().put("id", eventId).put("conversationId", row.getString("conversation_id"))
          .put("senderVoidId", sender).put("body", body).put("direction", "incoming").put("deliveryState", "received").put("createdAt", at))
        db.execSQL("INSERT INTO crypto_events(id,local_id,operation,outcome,plaintext,ciphertext,sender_identity,sender_signature_public_key,created_at) VALUES(?,?,'signal_decrypt','ok',?,?,?,?,?)",
          arrayOf(id(), identity().getString("localDeviceId"), plaintext, ciphertext, boundKey.serialize(), MessagingProtocol.bytes(person.getString("auth_public_key"), 32), now()))
        db.execSQL("INSERT INTO signal_inbox(sender_void_id,event_id,payload_hash,message_id) VALUES(?,?,?,?)", arrayOf(sender, eventId, digest, eventId))
        val receiptQueued = content.opt("v") == 2 && content.opt("request_delivery_receipt") == 1
        if (receiptQueued) saveReceiptObligation(sender, eventId)
        db.execSQL("UPDATE signal_threads SET last_error=NULL WHERE conversation_id=?", arrayOf(row.getString("conversation_id")))
        JSONObject().put("sender_void_id", sender).put("event_id", eventId).put("changed", true).apply {
          if (receiptQueued) put("receiptQueued", true)
        }
      } finally { plaintext.fill(0) }
    }
    diagnosticAfterCommit {
      SignalDecryptDiagnostics.logCommittedEvent(stage, ownId(), eventId, encoded)
    }
    result
  }

  /** Explicit, read-only diagnostics for this account's saved outgoing events. */
  fun logSendDiagnostics(conversationId: String): JSONObject {
    val localVoidId = identity().getString("accountVoidId")
    val row = thread(conversationId)
    val count = db.rawQuery(
      "SELECT o.event_id,o.payload_base64,o.created_at FROM signal_outbox o JOIN messages m ON m.id=o.event_id " +
        "WHERE m.conversation_id=? AND m.sender_void_id=? AND m.direction='outgoing' " +
        "ORDER BY o.created_at DESC,o.event_id DESC LIMIT 128",
      arrayOf(row.getString("conversation_id"), localVoidId)
    ).use { cursor ->
      var inspected = 0
      while (cursor.moveToNext()) {
        SignalDecryptDiagnostics.logStoredSend(cursor.getString(0), cursor.getString(1), cursor.getLong(2))
        inspected++
      }
      inspected
    }
    return JSONObject().put("count", count)
  }

  private fun validateAcceptance(operationId: String, response: Any) {
    val accepted = MessagingProtocol.objectValue(response)
    require(accepted.getString("type") == "accepted" && accepted.getString("event_id") == MessagingProtocol.uuid(operationId))
    require(RegistrationProof.timestampMillis(accepted.getString("expires_at")) > RegistrationProof.timestampMillis(accepted.getString("created_at")))
  }

  fun completeOperation(operationId: String, response: Any): JSONObject {
    validateAcceptance(operationId, response)
    storage.transaction {
      if (one("SELECT event_id FROM signal_outbox WHERE event_id=?", operationId) == null) throw SignalFailure("SIGNAL_OPERATION_MISSING")
      db.execSQL("UPDATE signal_outbox SET status='done',error_code=NULL WHERE event_id=?", arrayOf(operationId))
      db.execSQL("UPDATE messages SET delivery_state='sent' WHERE id=? AND delivery_state IN ('pending','failed')", arrayOf(operationId))
    }
    return state()
  }
  fun failOperation(operationId: String, errorCode: String): JSONObject {
    require(Regex("[a-z_]{1,64}").matches(errorCode))
    storage.transaction { failSavedMessage(MessagingProtocol.uuid(operationId), errorCode) }
    return state()
  }
  fun failPending(errorCode: String, peerVoidId: String? = null): JSONObject {
    identity()
    require(Regex("[a-z_]{1,64}").matches(errorCode))
    val target = peerVoidId?.let(MessagingProtocol::voidId)
    storage.transaction {
      val pending = rows(
        "SELECT m.id FROM messages m JOIN signal_threads t ON t.conversation_id=m.conversation_id WHERE m.direction='outgoing' AND m.delivery_state='pending' " +
          "AND (EXISTS (SELECT 1 FROM signal_drafts d WHERE d.message_id=m.id) OR EXISTS (SELECT 1 FROM signal_outbox o WHERE o.event_id=m.id AND o.status='pending'))" +
          (if (target == null) "" else " AND t.peer_void_id=?"),
        *(if (target == null) emptyArray() else arrayOf(target))
      )
      for (index in 0 until pending.length()) failSavedMessage(pending.getJSONObject(index).getString("id"), errorCode)
    }
    return state()
  }
  fun retryOperation(operationId: String): JSONObject {
    storage.transaction {
      MessagingProtocol.uuid(operationId)
      val message = one("SELECT * FROM messages WHERE id=?", operationId) ?: throw SignalFailure("SIGNAL_OPERATION_MISSING")
      require(message.getString("direction") == "outgoing" && message.getString("sender_void_id") == ownId())
      if (message.getString("delivery_state") != "failed") return@transaction
      val outbox = one("SELECT status FROM signal_outbox WHERE event_id=?", operationId)
      val draft = one("SELECT conversation_id FROM signal_drafts WHERE message_id=?", operationId)
      if (outbox == null && draft == null) throw SignalFailure("SIGNAL_OPERATION_MISSING")
      if (outbox?.getString("status") == "done") return@transaction
      val row = thread(message.getString("conversation_id"))
      available(row.getString("peer_void_id"))
      clearSendFailure(row)
      // Retry the same durable ciphertext and event ID; never re-encrypt.
      db.execSQL("UPDATE signal_outbox SET status='pending',error_code=NULL WHERE event_id=? AND status='failed'", arrayOf(operationId))
      db.execSQL("UPDATE messages SET delivery_state='pending' WHERE id=? AND delivery_state='failed'", arrayOf(operationId))
    }
    return state()
  }
  fun blockPeer(peerId: String): JSONObject {
    storage.transaction {
      val at = now()
      db.execSQL("INSERT OR IGNORE INTO messaging_blocks(peer_void_id,created_at) VALUES(?,?)", arrayOf(MessagingProtocol.voidId(peerId), at))
      db.execSQL("UPDATE signal_control_outbox SET status='stopped',payload_base64=NULL,next_attempt_at=? WHERE peer_void_id=? AND status IN ('ready','pending')",
        arrayOf(at, peerId))
    }
    return state()
  }
}
