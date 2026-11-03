package com.voidchats.storage

import com.voidchats.openmls.MlsEngine
import com.voidchats.openmls.MlsException
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject

/** Serialized by AppStorage. No key, plaintext or MLS checkpoint is sent to a server. */
internal class MessagingStore(private val storage: AppStorage) {
  private val db get() = storage.requireDatabase()
  private fun identity(): JSONObject = (storage.localIdentity() ?: throw StorageFailure("NOT_INITIALIZED")).also {
    if (it.isNull("accountVoidId") || it.isNull("accountApiUrl")) throw StorageFailure("NOT_INITIALIZED")
  }
  private fun ownId() = identity().getString("accountVoidId")
  private fun handle() = identity().getString("clientHandle")
  private fun id() = UUID.randomUUID().toString()
  private fun now() = System.currentTimeMillis()
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
  private fun json(text: String) = MessagingProtocol.objectValue(MessagingProtocol.parse(text))
  private fun <T> transaction(action: () -> T): T {
    db.beginTransaction()
    try { val result = action(); db.setTransactionSuccessful(); return result } finally { db.endTransaction() }
  }
  private fun diagnostic(stage: String, error: Throwable) {
    val code = error.javaClass.simpleName.replace(Regex("([a-z])([A-Z])"), "$1_$2").uppercase().replace(Regex("[^A-Z0-9_]"), "_")
    val safeStage = stage.replace(Regex("[^A-Za-z0-9_]"), "_").take(64)
    android.util.Log.w("VoidMessaging", "phase=$safeStage code=$code")
  }
  private fun <T> protocolMutation(stage: String = "protocol", action: (MlsEngine) -> T): T = storage.mutateMls { engine ->
    // Malformed authenticated transport data is a protocol rejection, never a
    // local database failure. Preserve any already checkpointed receive ratchet.
    try { action(engine) }
    catch (error: IllegalArgumentException) { diagnostic(stage, error); throw MlsException.InvalidMessage() }
    catch (error: org.json.JSONException) { diagnostic(stage, error); throw MlsException.InvalidMessage() }
    catch (error: MlsException) { diagnostic(stage, error); throw error }
  }
  private fun blocked(peer: String) = one("SELECT peer_void_id FROM messaging_blocks WHERE peer_void_id=?", peer) != null
  private fun available(peer: String) { if (blocked(peer)) throw StorageFailure("CONFLICT") }
  private fun session(sessionId: String): JSONObject = one("SELECT * FROM messaging_sessions WHERE thread_id=? OR id=? ORDER BY CASE WHEN thread_id=? THEN 0 ELSE 1 END LIMIT 1", MessagingProtocol.uuid(sessionId), sessionId, sessionId) ?: throw StorageFailure("NOT_FOUND")
  private fun wireSession(sessionId: String): JSONObject = one("SELECT * FROM messaging_sessions WHERE id=?", MessagingProtocol.uuid(sessionId)) ?: throw MlsException.InvalidMessage()
  private fun threadId(row: JSONObject) = if (row.isNull("thread_id")) row.getString("id") else row.getString("thread_id")
  private fun operation(operationId: String): JSONObject = one("SELECT * FROM messaging_operations WHERE id=?", MessagingProtocol.uuid(operationId)) ?: throw StorageFailure("NOT_FOUND")
  private fun peer(row: JSONObject) = JSONObject().put("void_id", row.getString("peer_void_id")).put("auth_public_key", row.getString("peer_key_base64"))
  private fun publicOperation(row: JSONObject) = JSONObject().put("id", row.getString("id")).put("sessionId", row.getString("session_id"))
    .put("kind", row.getString("kind")).put("peerVoidId", row.getString("peer_void_id")).put("payloadBase64", row.getString("payload_base64"))
    .put("purpose", row.getString("purpose"))
    .put("status", row.getString("status")).apply { if (!row.isNull("error_code")) put("errorCode", row.getString("error_code")) }
  fun state(): JSONObject {
    identity(); recoverInbox()
    val sessions = rows("SELECT * FROM messaging_sessions WHERE thread_id IS NOT NULL ORDER BY created_at DESC,id").let { data -> JSONArray().apply {
      for (index in 0 until data.length()) {
        val row = data.getJSONObject(index)
        val status = when (row.getString("status")) {
          "requested", "offered" -> "pending"
          "awaiting_welcome", "awaiting_ready", "awaiting_ack" -> "handshaking"
          else -> row.getString("status")
        }
        val hasMessages = one("SELECT id FROM messages WHERE conversation_id=? LIMIT 1", threadId(row)) != null
        put(JSONObject().put("id", threadId(row)).put("peer", peer(row)).put("direction", row.getString("direction"))
          .put("status", status).put("createdAt", row.getLong("created_at")).put("isRequest", row.getInt("is_request") == 1)
          .put("hasMessages", hasMessages).put("diagnosticCode", row.opt("last_error") ?: JSONObject.NULL))
      }
    } }
    val operations = rows("SELECT * FROM messaging_operations WHERE status<>'done' ORDER BY created_at,rowid").let { data -> JSONArray().apply { for (index in 0 until data.length()) put(publicOperation(data.getJSONObject(index))) } }
    val blocks = rows("SELECT peer_void_id FROM messaging_blocks ORDER BY peer_void_id").let { data -> JSONArray().apply { for (index in 0 until data.length()) put(data.getJSONObject(index).getString("peer_void_id")) } }
    return JSONObject().put("sessions", sessions).put("outbox", operations).put("blockedVoidIds", blocks)
  }
  fun signAuthChallenge(messageBase64: String): String {
    val local = identity()
    return MessagingProtocol.base64(storage.requireEngine().signAuthChallenge(local.getString("clientHandle"), MessagingProtocol.authMessage(messageBase64, local)))
  }
  private fun envelope(kind: String, row: JSONObject): JSONObject = JSONObject().put("v", 1).put("kind", kind).put("conversation_id", row.getString("id"))
    .put("from_void_id", ownId()).put("to_void_id", row.getString("peer_void_id"))
  private fun addOperation(kind: String, row: JSONObject, payload: JSONObject, operationId: String = id()): String {
    val bytes = payload.toString().toByteArray(Charsets.UTF_8)
    require(bytes.size in 1..1024 * 1024)
    val encoded = try { MessagingProtocol.base64(bytes) } finally { bytes.fill(0) }
    db.execSQL("INSERT INTO messaging_operations(id,session_id,kind,purpose,peer_void_id,payload_base64,status,created_at) VALUES(?,?,'message',?,?,?,'pending',?)",
      arrayOf<Any>(operationId, row.getString("id"), if (kind == "relay") "setup" else "application", row.getString("peer_void_id"), encoded, now()))
    return operationId
  }
  private fun copySetup(row: JSONObject): String {
    require(row.getString("purpose") == "setup")
    val pending = one("SELECT id FROM messaging_operations WHERE session_id=? AND purpose='setup' AND payload_base64=? AND status='pending' LIMIT 1", row.getString("session_id"), row.getString("payload_base64"))
    if (pending != null) {
      if (pending.getString("id") != row.getString("id")) db.execSQL("UPDATE messaging_operations SET status='done',error_code=NULL WHERE id=?", arrayOf(row.getString("id")))
      return pending.getString("id")
    }
    val operationId = id()
    db.execSQL("INSERT INTO messaging_operations(id,session_id,kind,purpose,peer_void_id,payload_base64,status,created_at) VALUES(?,?,'message','setup',?,?,'pending',?)", arrayOf<Any>(operationId, row.getString("session_id"), row.getString("peer_void_id"), row.getString("payload_base64"), now()))
    db.execSQL("UPDATE messaging_operations SET status='done',error_code=NULL WHERE id=?", arrayOf(row.getString("id")))
    return operationId
  }
  private fun retryLast(sessionId: String, explicit: Boolean = false) {
    val last = one("SELECT * FROM messaging_operations WHERE session_id=? AND purpose='setup' ORDER BY rowid DESC LIMIT 1", sessionId)
    // Queued setup already persists on the server. It must not be resent merely
    // because the peer is offline; that is not a failed operation.
    if (last != null && last.getString("status") == "failed" && last.optString("error_code") == "duplicate_event") copySetup(last)
    else if (last != null && last.getString("status") == "failed") db.execSQL("UPDATE messaging_operations SET status='pending',error_code=NULL WHERE id=?", arrayOf(last.getString("id")))
    else if (last != null && explicit && last.getString("status") == "done") copySetup(last)
  }
  private fun pin(peer: JSONObject) {
    MessagingProtocol.person(peer); require(peer.getString("void_id") != ownId()); available(peer.getString("void_id"))
    val contact = one("SELECT identity_public_key FROM contacts WHERE void_id=?", peer.getString("void_id"))
    if (contact != null && contact.getString("identity_public_key") != peer.getString("auth_public_key")) throw StorageFailure("PEER_KEY_CHANGED")
  }
  private fun ensureConversation(row: JSONObject) {
    val old = one("SELECT title,created_at,updated_at FROM conversations WHERE id=?", threadId(row))
    val createdAt = old?.getLong("created_at") ?: row.getLong("created_at")
    PublicRepositories(db).execute("conversations.save", JSONObject().put("id", threadId(row)).put("kind", "direct")
      .put("title", old?.getString("title") ?: row.getString("peer_void_id")).put("peerVoidId", row.getString("peer_void_id"))
      .put("mlsGroupId", row.getString("group_id_base64")).put("createdAt", createdAt).put("updatedAt", maxOf(createdAt, old?.getLong("updated_at") ?: createdAt)))
  }
  /** Opening a composer creates only a local thread, never a server request. */
  fun openConversation(peer: JSONObject): JSONObject {
    identity(); pin(peer)
    val row = transaction {
      val old = one("SELECT * FROM messaging_sessions WHERE peer_void_id=? AND status NOT IN ('declined','blocked')", peer.getString("void_id"))
      val result = if (old != null) {
        if (old.getString("peer_key_base64") != peer.getString("auth_public_key")) throw StorageFailure("PEER_KEY_CHANGED")
        old
      } else {
        if (one("SELECT id FROM conversations WHERE peer_void_id=?", peer.getString("void_id")) != null) throw StorageFailure("CONFLICT")
        val sessionId = id()
        db.execSQL("INSERT INTO messaging_sessions(id,thread_id,peer_void_id,peer_key_base64,direction,status,group_id_base64,created_at,is_request) VALUES(?,?,?,?,'outgoing','requested',?,?,0)",
          arrayOf<Any>(sessionId, sessionId, peer.getString("void_id"), peer.getString("auth_public_key"), MessagingProtocol.base64(sessionId.toByteArray(Charsets.UTF_8)), now()))
        wireSession(sessionId)
      }
      ensureConversation(result)
      result
    }
    return MessagingProtocol.objectValue(MessagingProtocol.parse(PublicRepositories(db).execute("conversations.get", JSONObject().put("id", threadId(row))).toString()))
  }
  private fun startRequest(row: JSONObject) {
    if (row.getString("status") == "requested" && one("SELECT id FROM messaging_operations WHERE session_id=? AND purpose='setup' LIMIT 1", row.getString("id")) == null) {
      addOperation("relay", row, envelope("key_package_request", row))
    }
  }
  /** Kept for older JS builds; current UI opens a composer and sends directly. */
  fun beginRequest(peer: JSONObject): JSONObject {
    val conversation = openConversation(peer)
    transaction { startRequest(session(conversation.getString("id"))) }
    return resumePending()
  }
  private fun offerPackage(row: JSONObject, engine: MlsEngine): String {
    require(row.getString("status") == "offered")
    val packageId = id(); val encoded = MessagingProtocol.base64(engine.generateKeyPackage(handle()))
    db.execSQL("INSERT INTO key_packages(id,client_handle,package_base64,created_at) VALUES(?,?,?,?)", arrayOf<Any>(packageId, handle(), encoded, now()))
    db.execSQL("UPDATE messaging_sessions SET status='awaiting_welcome',package_id=?,package_base64=?,last_error=NULL WHERE id=?", arrayOf(packageId, encoded, row.getString("id")))
    return addOperation("relay", row, envelope("key_package", row).put("key_package_base64", encoded))
  }
  /** Automatic local work resumes while the network or peer is unavailable. */
  fun resumePending(): JSONObject {
    identity(); recoverInbox()
    val pending = rows("SELECT * FROM messaging_sessions WHERE thread_id IS NOT NULL AND status NOT IN ('declined','blocked') ORDER BY created_at,id")
    for (index in 0 until pending.length()) {
      val row = pending.getJSONObject(index)
      if (row.optString("last_error") == "PEER_KEY_CHANGED") continue
      try {
        if (row.optString("last_error").startsWith("TRANSPORT_")) transaction { retryLast(row.getString("id")) }
        if (row.getString("status") == "offered") protocolMutation { engine -> offerPackage(row, engine) }
        else if (row.getString("status") == "requested" && one("SELECT message_id FROM messaging_drafts WHERE session_id=? LIMIT 1", row.getString("id")) != null) transaction { startRequest(row) }
        if (wireSession(row.getString("id")).getString("status") == "active") flushDrafts(wireSession(row.getString("id")))
      } catch (error: MlsException) {
        if (error is MlsException.StorageFailure || error is MlsException.InvalidState) throw error
        pause(row, "MLS_" + error.javaClass.simpleName.uppercase(), error)
      } catch (error: IllegalArgumentException) { pause(row, "INVALID_MESSAGE_STATE", error) }
      catch (error: StorageFailure) {
        if (error.code !in setOf("CONFLICT", "NOT_FOUND", "INVALID_INPUT", "PEER_KEY_CHANGED")) throw error
        pause(row, error.code, error)
      }
    }
    return state()
  }
  private fun pause(row: JSONObject, code: String, error: Throwable) {
    diagnostic("resume_message", error)
    transaction {
      db.execSQL("UPDATE messaging_sessions SET last_error=? WHERE id=?", arrayOf(code, row.getString("id")))
      db.execSQL("UPDATE messages SET delivery_state='failed' WHERE delivery_state='pending' AND id IN (SELECT message_id FROM messaging_drafts WHERE session_id=?)", arrayOf(row.getString("id")))
    }
  }
  /** Compatibility only. Consent is not a messaging prerequisite anymore. */
  fun prepareRequestAction(sessionId: String, action: String): JSONObject {
    identity(); require(action in setOf("accepted", "declined"))
    val row = session(sessionId); available(row.getString("peer_void_id"))
    if (action == "declined") return blockPeer(row.getString("peer_void_id"))
    if (row.getString("status") == "offered") protocolMutation { engine -> offerPackage(row, engine) }
    return resumePending()
  }
  fun blockPeer(peerId: String): JSONObject {
    identity(); MessagingProtocol.voidId(peerId); require(peerId != ownId())
    transaction {
      db.execSQL("INSERT OR IGNORE INTO messaging_blocks(peer_void_id,created_at) VALUES(?,?)", arrayOf(peerId, now()))
      db.execSQL("UPDATE messaging_sessions SET status='blocked' WHERE peer_void_id=?", arrayOf(peerId))
      db.execSQL("UPDATE messaging_operations SET status='failed',error_code='peer_blocked' WHERE peer_void_id=? AND status='pending'", arrayOf(peerId))
    }
    return state()
  }
  private fun group(row: JSONObject) = MessagingProtocol.bytes(row.getString("group_id_base64"), 256)
  private fun peerKey(row: JSONObject) = MessagingProtocol.bytes(row.getString("peer_key_base64"), 32)
  private fun peerCredential(row: JSONObject) = MessagingProtocol.bytes(row.getString("peer_credential_base64"), 1024)
  private fun checkedGroup(engine: MlsEngine, row: JSONObject) = engine.checkDirectGroup(handle(), group(row), peerKey(row), peerCredential(row))
  private fun activate(row: JSONObject, engine: MlsEngine) {
    checkedGroup(engine, row)
    require(row.getString("status") in setOf("awaiting_ready", "awaiting_ack", "active"))
    val peerId = row.getString("peer_void_id"); val createdAt = row.getLong("created_at")
    val oldContact = one("SELECT id,display_name,created_at FROM contacts WHERE void_id=?", peerId)
    val contactId = oldContact?.getString("id") ?: peerId
    PublicRepositories(db).execute("contacts.save", JSONObject().put("id", contactId).put("voidId", peerId).put("displayName", oldContact?.getString("display_name") ?: "VOID-$peerId")
      .put("identityPublicKey", row.getString("peer_key_base64")).put("createdAt", oldContact?.getLong("created_at") ?: createdAt).put("updatedAt", now()))
    ensureConversation(row)
    db.execSQL("UPDATE messaging_sessions SET status='active',last_error=NULL WHERE id=?", arrayOf(row.getString("id")))
  }
  private fun ready(engine: MlsEngine, row: JSONObject, kind: String): JSONObject {
    checkedGroup(engine, row)
    val plain = envelope(kind, row).toString().toByteArray(Charsets.UTF_8)
    val encrypted = try { engine.encrypt(handle(), group(row), plain) } finally { plain.fill(0) }
    return envelope(kind, row).put("ciphertext_base64", MessagingProtocol.base64(encrypted))
  }
  private fun verifyReady(engine: MlsEngine, row: JSONObject, payload: JSONObject, kind: String) {
    checkedGroup(engine, row)
    val decrypted = engine.decrypt(handle(), group(row), MessagingProtocol.bytes(MessagingProtocol.text(payload, "ciphertext_base64", 1_398_104)))
    try {
      require(decrypted.senderSignaturePublicKey.contentEquals(peerKey(row)) && decrypted.senderIdentity.contentEquals(peerCredential(row)))
      val text = decrypted.plaintext.toString(Charsets.UTF_8)
      require(text.toByteArray(Charsets.UTF_8).contentEquals(decrypted.plaintext))
      val expected = envelope(kind, row).put("from_void_id", row.getString("peer_void_id")).put("to_void_id", ownId())
      require(MessagingProtocol.canonical(json(text)) == MessagingProtocol.canonical(expected))
    } finally { decrypted.plaintext.fill(0) }
  }
  /** Directory keys are supplied by the authenticated socket, never by an envelope. */
  private fun applyControl(sender: String, exchangeId: String, payloadBase64: String, registeredPeer: JSONObject) {
    identity(); MessagingProtocol.voidId(sender); MessagingProtocol.person(registeredPeer)
    require(Regex("[A-Za-z0-9._:-]{1,64}").matches(exchangeId) && registeredPeer.getString("void_id") == sender)
    pin(registeredPeer)
    val payload = decodePayload(payloadBase64); val digest = MessagingProtocol.hash(payloadBase64)
    val kind = MessagingProtocol.text(payload, "kind", 32); val sessionId = MessagingProtocol.uuid(MessagingProtocol.text(payload, "conversation_id", 36))
    validateEnvelope(payload, sender)
    val seen = one("SELECT * FROM messaging_relays WHERE sender_void_id=? AND exchange_id=?", sender, exchangeId)
    if (seen != null) {
      require(seen.getString("payload_hash") == digest)
      transaction {
        val response = replayControlResponse(seen, sessionId, kind, sender, registeredPeer)
        if (response != null) db.execSQL("UPDATE messaging_relays SET response_operation_id=? WHERE sender_void_id=? AND exchange_id=?", arrayOf(response, sender, exchangeId))
      }
      return
    }
    // Explicit retry may use a new transport UUID for the exact stored setup
    // bytes. Never decrypt an already-consumed ready/Welcome a second time.
    val samePayload = one("SELECT response_operation_id FROM messaging_relays WHERE sender_void_id=? AND payload_hash=? LIMIT 1", sender, digest)
    if (samePayload != null) {
      transaction {
        val response = replayControlResponse(samePayload, sessionId, kind, sender, registeredPeer)
        db.execSQL("INSERT INTO messaging_relays(sender_void_id,exchange_id,payload_hash,response_operation_id) VALUES(?,?,?,?)", arrayOf(sender, exchangeId, digest, response))
      }
      return
    }
    protocolMutation("receive_" + kind) { engine ->
      var row = one("SELECT * FROM messaging_sessions WHERE id=?", sessionId)
      var response: String? = null
      if (kind == "key_package_request") {
        if (row == null) {
          val other = one("SELECT * FROM messaging_sessions WHERE peer_void_id=? AND status NOT IN ('declined','blocked')", sender)
          var thread = sessionId
          var request = 1
          if (other != null) {
            val started = one("SELECT id FROM messaging_operations WHERE session_id=? AND purpose='setup' LIMIT 1", other.getString("id")) != null
            if (other.getString("status") == "active" || (other.getString("direction") == "outgoing" && other.getString("status") == "requested" && started && ownId().toLong() < sender.toLong())) {
              db.execSQL("INSERT INTO messaging_relays(sender_void_id,exchange_id,payload_hash,response_operation_id) VALUES(?,?,?,NULL)", arrayOf(sender, exchangeId, digest))
              return@protocolMutation
            }
            require(other.getString("direction") == "outgoing" && other.getString("status") == "requested")
            thread = threadId(other)
            request = if (one("SELECT id FROM messages WHERE conversation_id=? AND direction='outgoing' LIMIT 1", thread) == null) 1 else other.getInt("is_request")
            db.execSQL("UPDATE messaging_sessions SET status='declined',thread_id=NULL WHERE id=?", arrayOf(other.getString("id")))
            db.execSQL("UPDATE messaging_operations SET status='done',error_code=NULL WHERE session_id=? AND purpose='setup'", arrayOf(other.getString("id")))
          } else require(one("SELECT id FROM conversations WHERE peer_void_id=?", sender) == null)
          db.execSQL("INSERT INTO messaging_sessions(id,thread_id,peer_void_id,peer_key_base64,direction,status,group_id_base64,created_at,is_request) VALUES(?,?,?,?,'incoming','offered',?,?,?)",
            arrayOf<Any>(sessionId, thread, sender, registeredPeer.getString("auth_public_key"), MessagingProtocol.base64(sessionId.toByteArray(Charsets.UTF_8)), now(), request))
          if (other != null) db.execSQL("UPDATE messaging_drafts SET session_id=? WHERE session_id=?", arrayOf(sessionId, other.getString("id")))
          row = wireSession(sessionId)
          ensureConversation(row)
        } else {
          require(row.getString("direction") == "incoming" && row.getString("peer_void_id") == sender && row.getString("peer_key_base64") == registeredPeer.getString("auth_public_key"))
        }
        if (row.getString("status") == "offered") response = offerPackage(row, engine)
      } else {
        row = row ?: throw MlsException.InvalidMessage()
        require(row.getString("peer_void_id") == sender && row.getString("peer_key_base64") == registeredPeer.getString("auth_public_key"))
        when (kind) {
          "declined" -> {
            // Compatibility with an older peer. No consent step is sent by the new client.
            require(row.getString("direction") == "outgoing" && row.getString("status") == "requested")
            db.execSQL("UPDATE messaging_sessions SET last_error='PEER_CLIENT_UPDATE_REQUIRED' WHERE id=?", arrayOf(sessionId))
          }
          "key_package" -> {
            require(row.getString("direction") == "outgoing" && row.getString("status") == "requested")
            val packageBytes = MessagingProtocol.bytes(MessagingProtocol.text(payload, "key_package_base64", 1_398_104))
            val credential = engine.validateKeyPackage(packageBytes, peerKey(row))
            // A prior operation can checkpoint an empty group before an Add
            // fails. Reuse that exact group instead of stranding the thread.
            val info = try { engine.groupInfo(handle(), group(row)) } catch (_: MlsException.UnknownGroup) { null }
            val welcome: String
            if (info != null && info.memberCount == 1U && !row.isNull("welcome_base64")) {
              engine.mergePendingCommit(handle(), group(row))
              welcome = row.getString("welcome_base64")
            } else if (info == null || info.memberCount == 1U) {
              if (info == null) engine.createGroup(handle(), group(row))
              val added = engine.addMember(handle(), group(row), packageBytes)
              welcome = MessagingProtocol.base64(added.welcome)
              db.execSQL("UPDATE messaging_sessions SET peer_credential_base64=?,welcome_base64=? WHERE id=?", arrayOf(MessagingProtocol.base64(credential), welcome, sessionId))
              engine.mergePendingCommit(handle(), group(row))
            } else {
              require(info.memberCount == 2U && !row.isNull("welcome_base64"))
              welcome = row.getString("welcome_base64")
            }
            db.execSQL("UPDATE messaging_sessions SET status='awaiting_ready',peer_credential_base64=?,welcome_base64=?,last_error=NULL WHERE id=?", arrayOf(MessagingProtocol.base64(credential), welcome, sessionId))
            row = wireSession(sessionId); checkedGroup(engine, row)
            response = addOperation("relay", row, envelope("welcome", row).put("welcome_base64", welcome))
          }
          "welcome" -> {
            require(row.getString("direction") == "incoming" && row.getString("status") == "awaiting_welcome")
            val welcome = MessagingProtocol.text(payload, "welcome_base64", 1_398_104)
            val joined = engine.processWelcomeChecked(handle(), MessagingProtocol.bytes(welcome), group(row), peerKey(row))
            db.execSQL("UPDATE messaging_sessions SET status='awaiting_ack',peer_credential_base64=?,welcome_base64=?,last_error=NULL WHERE id=?", arrayOf(MessagingProtocol.base64(joined.peerCredential), welcome, sessionId))
            db.execSQL("UPDATE key_packages SET consumed_at=? WHERE id=?", arrayOf<Any>(now(), row.getString("package_id")))
            row = wireSession(sessionId)
            response = addOperation("relay", row, ready(engine, row, "ready"))
          }
          "ready" -> {
            require(row.getString("direction") == "outgoing" && row.getString("status") == "awaiting_ready")
            verifyReady(engine, row, payload, "ready")
            response = addOperation("relay", row, ready(engine, row, "ready_ack"))
            activate(row, engine)
          }
          "ready_ack" -> {
            require(row.getString("direction") == "incoming" && row.getString("status") in setOf("awaiting_ack", "active"))
            verifyReady(engine, row, payload, "ready_ack")
            activate(row, engine)
          }
          else -> throw IllegalArgumentException()
        }
      }
      // A verified next stage proves the peer consumed the prior setup. Stop
      // retrying old stages, while retaining their exact bytes for recovery.
      db.execSQL("UPDATE messaging_operations SET status='done',error_code=NULL WHERE session_id=? AND purpose='setup' AND id<>?", arrayOf(sessionId, response ?: ""))
      db.execSQL("INSERT INTO messaging_relays(sender_void_id,exchange_id,payload_hash,response_operation_id) VALUES(?,?,?,?)", arrayOf(sender, exchangeId, digest, response))
    }
  }
  private fun replayControlResponse(seen: JSONObject, sessionId: String, kind: String, sender: String, registeredPeer: JSONObject): String? {
    val bound = one("SELECT * FROM messaging_sessions WHERE id=?", sessionId) ?: return null
    require(bound.getString("peer_void_id") == sender && bound.getString("peer_key_base64") == registeredPeer.getString("auth_public_key"))
    val reply = if (!seen.isNull("response_operation_id")) operation(seen.getString("response_operation_id"))
      else if (kind == "key_package_request" && bound.getString("direction") == "incoming" && bound.getString("status") !in setOf("offered", "blocked"))
        one("SELECT * FROM messaging_operations WHERE session_id=? AND purpose='setup' ORDER BY rowid DESC LIMIT 1", sessionId)
      else null
    // In older builds a forwarded ready_ack could disappear before Bob saved
    // it. Requeue those already encrypted bytes; never consume Ready twice.
    return reply?.let { copySetup(it) }
  }
  fun applyOnline(sender: String, exchangeId: String, payloadBase64: String, peer: JSONObject): JSONObject {
    applyControl(sender, exchangeId, payloadBase64, peer)
    return resumePending()
  }
  private fun decodePayload(encoded: String): JSONObject {
    val bytes = MessagingProtocol.bytes(encoded)
    return try {
      val text = bytes.toString(Charsets.UTF_8)
      require(text.toByteArray(Charsets.UTF_8).contentEquals(bytes))
      json(text)
    } finally { bytes.fill(0) }
  }
  private fun validateEnvelope(payload: JSONObject, sender: String) {
    require(payload.opt("v") == 1 && MessagingProtocol.text(payload, "from_void_id", 19) == sender && MessagingProtocol.text(payload, "to_void_id", 19) == ownId())
  }
  fun queueMessage(conversationId: String, body: String): JSONObject {
    identity(); require(body.isNotBlank() && !body.contains('\u0000') && body.toByteArray(Charsets.UTF_8).toString(Charsets.UTF_8) == body)
    val row = session(conversationId); available(row.getString("peer_void_id")); require(row.getString("status") !in setOf("declined", "blocked"))
    val messageId = id(); val createdAt = now()
    // The draft itself is encrypted by SQLCipher. It cannot leave the device
    // until OpenMLS has verified and established the peer-bound group.
    require(body.toByteArray(Charsets.UTF_8).size <= 60_000)
    transaction {
      ensureConversation(row)
      PublicRepositories(db).execute("messages.save", JSONObject().put("id", messageId).put("conversationId", threadId(row)).put("senderVoidId", ownId()).put("body", body)
        .put("direction", "outgoing").put("deliveryState", "pending").put("createdAt", createdAt))
      db.execSQL("INSERT INTO messaging_drafts(message_id,session_id) VALUES(?,?)", arrayOf(messageId, row.getString("id")))
      db.execSQL("UPDATE messaging_sessions SET is_request=0 WHERE id=?", arrayOf(row.getString("id")))
      startRequest(row)
    }
    return resumePending()
  }
  private fun flushDrafts(row: JSONObject) {
    val drafts = rows("SELECT m.* FROM messaging_drafts d JOIN messages m ON m.id=d.message_id WHERE d.session_id=? ORDER BY m.created_at,m.id", row.getString("id"))
    for (index in 0 until drafts.length()) {
      val draft = drafts.getJSONObject(index)
      val messageId = draft.getString("id")
      val content = JSONObject().put("v", 1).put("client_message_id", messageId).put("conversation_id", row.getString("id"))
        .put("from_void_id", ownId()).put("to_void_id", row.getString("peer_void_id")).put("body", draft.getString("body")).put("created_at", draft.getLong("created_at")).toString().toByteArray(Charsets.UTF_8)
      require(content.size <= 65_536)
      try {
        protocolMutation("encrypt_application") { engine ->
          checkedGroup(engine, row)
          val ciphertext = engine.encrypt(handle(), group(row), content)
          val payload = envelope("application", row).put("client_message_id", messageId).put("ciphertext_base64", MessagingProtocol.base64(ciphertext))
          addOperation("message", row, payload, messageId)
          db.execSQL("DELETE FROM messaging_drafts WHERE message_id=?", arrayOf(messageId))
          db.execSQL("UPDATE messages SET delivery_state='pending' WHERE id=? AND delivery_state='failed'", arrayOf(messageId))
          db.execSQL("UPDATE messaging_sessions SET last_error=NULL WHERE id=?", arrayOf(row.getString("id")))
          storage.journal("encrypt", group(row), null, ciphertext)
        }
      } finally { content.fill(0) }
    }
  }
  fun completeOperation(operationId: String, response: Any): JSONObject {
    identity(); val row = operation(operationId)
    val objectResponse = MessagingProtocol.objectValue(response)
    if (row.getString("purpose") == "setup" && objectResponse.optString("type") == "online_result") {
      require(objectResponse.optString("type") == "online_result" && objectResponse.optString("exchange_id") == operationId)
      val status = objectResponse.optString("status")
      require(status in setOf("forwarded", "unavailable", "overloaded", "failed"))
      if (status != "forwarded") return failOperation(operationId, if (status == "unavailable") "peer_offline" else status)
    }
    transaction {
      if (row.getString("purpose") == "setup" && objectResponse.optString("type") == "online_result") {
        require(objectResponse.optString("type") == "online_result" && objectResponse.optString("exchange_id") == operationId && objectResponse.optString("status") == "forwarded")
      } else {
        require(objectResponse.optString("type") == "accepted" && objectResponse.optString("event_id") == operationId)
        val createdAt = RegistrationProof.timestampMillis(MessagingProtocol.text(objectResponse, "created_at", 40))
        require(RegistrationProof.timestampMillis(MessagingProtocol.text(objectResponse, "expires_at", 40)) > createdAt)
        if (row.getString("purpose") == "application") db.execSQL("UPDATE messages SET delivery_state='sent' WHERE id=? AND delivery_state IN ('pending','failed')", arrayOf(operationId))
        else db.execSQL("UPDATE messaging_sessions SET last_error=NULL WHERE id=? AND last_error LIKE 'TRANSPORT_%'", arrayOf(row.getString("session_id")))
      }
      db.execSQL("UPDATE messaging_operations SET status='done',error_code=NULL WHERE id=?", arrayOf(operationId))
    }
    return state()
  }
  fun failOperation(operationId: String, errorCode: String): JSONObject {
    identity(); require(Regex("[a-z][a-z0-9_]{0,79}").matches(errorCode)); val row = operation(operationId)
    transaction {
      if (row.getString("status") != "done") {
        // Setup has content-hash deduplication at the peer, unlike application
        // sends. A lost acceptance can safely retry the SAME encrypted control
        // bytes under a fresh transport ID without repeating an MLS mutation.
        if (row.getString("purpose") == "setup" && errorCode == "duplicate_event") {
          db.execSQL("UPDATE messaging_operations SET status='done',error_code=NULL WHERE id=?", arrayOf(operationId))
          copySetup(operation(operationId))
          return@transaction
        }
        db.execSQL("UPDATE messaging_operations SET status='failed',error_code=? WHERE id=?", arrayOf(errorCode, operationId))
        if (row.getString("purpose") == "application") db.execSQL("UPDATE messages SET delivery_state='failed' WHERE id=? AND delivery_state='pending'", arrayOf(operationId))
        else db.execSQL("UPDATE messaging_sessions SET last_error=? WHERE id=?", arrayOf("TRANSPORT_" + errorCode.uppercase(), row.getString("session_id")))
      }
    }
    return state()
  }
  fun retryOperation(operationId: String): JSONObject {
    identity()
    val row = one("SELECT * FROM messaging_operations WHERE id=?", MessagingProtocol.uuid(operationId))
    if (row == null) {
      val draft = one("SELECT session_id FROM messaging_drafts WHERE message_id=?", operationId) ?: throw StorageFailure("NOT_FOUND")
      val pending = wireSession(draft.getString("session_id")); available(pending.getString("peer_void_id"))
      transaction {
        db.execSQL("UPDATE messages SET delivery_state='pending' WHERE id=? AND delivery_state='failed'", arrayOf(operationId))
        startRequest(pending); retryLast(pending.getString("id"), explicit = true)
      }
      return resumePending()
    }
    available(row.getString("peer_void_id"))
    // duplicate_event cannot prove the original ciphertext. Preserve ambiguity;
    // never mint another UUID/ciphertext automatically after a lost acceptance.
    require(row.getString("purpose") == "setup" || row.optString("error_code") != "duplicate_event")
    transaction {
      if (row.getString("purpose") == "setup" && row.optString("error_code") == "duplicate_event") {
        copySetup(row)
        return@transaction
      }
      if (row.getString("status") == "failed") {
        db.execSQL("UPDATE messaging_operations SET status='pending',error_code=NULL WHERE id=?", arrayOf(operationId))
        if (row.getString("purpose") == "application") db.execSQL("UPDATE messages SET delivery_state='pending' WHERE id=? AND delivery_state='failed'", arrayOf(operationId))
      }
    }
    return state()
  }
  /** Returns an ACK token only after the ratchet, journal AND visible history commit. */
  fun applyEvent(event: JSONObject, registeredPeer: JSONObject? = null): JSONObject {
    identity(); require(event.optString("type") == "event")
    val sender = MessagingProtocol.voidId(MessagingProtocol.text(event, "from_void_id", 19))
    val eventId = MessagingProtocol.uuid(MessagingProtocol.text(event, "event_id", 36))
    val encoded = MessagingProtocol.text(event, "payload_base64", 1_398_104); MessagingProtocol.bytes(encoded).fill(0)
    val createdAt = RegistrationProof.timestampMillis(MessagingProtocol.text(event, "created_at", 40))
    require(RegistrationProof.timestampMillis(MessagingProtocol.text(event, "expires_at", 40)) > createdAt)
    val digest = MessagingProtocol.hash(encoded)
    val old = one("SELECT * FROM messaging_inbox WHERE sender_void_id=? AND event_id=?", sender, eventId)
    if (old != null) {
      require(old.getString("payload_hash") == digest)
      val recovered = old.getString("outcome") == "staged"
      recoverInbox()
      return JSONObject().put("sender_void_id", sender).put("event_id", eventId).put("changed", recovered)
    }
    val payload = try { decodePayload(encoded) } catch (_: IllegalArgumentException) { null }
    if (payload != null && payload.optString("kind") != "application") {
      var outcome = "applied"
      var diagnosticCode: String? = null
      try {
        applyControl(sender, eventId, encoded, registeredPeer ?: throw IllegalArgumentException())
      } catch (error: MlsException) {
        if (error is MlsException.StorageFailure || error is MlsException.InvalidState) throw error
        diagnostic("receive_control", error); outcome = "rejected"; diagnosticCode = "MLS_" + error.javaClass.simpleName.uppercase()
      } catch (error: IllegalArgumentException) {
        diagnostic("receive_control", error); outcome = "rejected"; diagnosticCode = "INVALID_CONTROL"
      } catch (error: StorageFailure) {
        if (error.code !in setOf("CONFLICT", "NOT_FOUND", "INVALID_INPUT", "PEER_KEY_CHANGED")) throw error
        diagnostic("receive_control", error); outcome = "rejected"; diagnosticCode = error.code
      }
      transaction {
        val related = payload.optString("conversation_id")
        db.execSQL("INSERT INTO messaging_inbox(sender_void_id,event_id,payload_hash,session_id,journal_id,outcome,processed_at) VALUES(?,?,?,?,NULL,?,?)", arrayOf<Any?>(sender, eventId, digest, null, outcome, now()))
        if (diagnosticCode != null) db.execSQL("UPDATE messaging_sessions SET last_error=? WHERE id=? AND peer_void_id=?", arrayOf(diagnosticCode, related, sender))
      }
      resumePending()
      return JSONObject().put("sender_void_id", sender).put("event_id", eventId).put("changed", outcome == "applied").put("diagnosticCode", diagnosticCode ?: JSONObject.NULL)
    }
    storage.mutateMls { engine ->
      var journalId: String? = null; var sessionId: String? = null; var outcome = "rejected"
      var rejectedCode: String? = null
      try {
        require(sender != ownId()); available(sender)
        val payload = decodePayload(encoded); validateEnvelope(payload, sender)
        require(payload.optString("kind") == "application" && payload.optString("client_message_id") == eventId)
        sessionId = MessagingProtocol.uuid(MessagingProtocol.text(payload, "conversation_id", 36))
        val row = wireSession(sessionId)
        require(row.getString("peer_void_id") == sender && row.getString("status") in setOf("active", "awaiting_ack"))
        if (registeredPeer != null) {
          MessagingProtocol.person(registeredPeer)
          require(registeredPeer.getString("void_id") == sender)
          if (registeredPeer.getString("auth_public_key") != row.getString("peer_key_base64")) throw StorageFailure("PEER_KEY_CHANGED")
        }
        val ciphertext = MessagingProtocol.bytes(MessagingProtocol.text(payload, "ciphertext_base64", 1_398_104))
        val decrypted = engine.decryptDirectMessage(handle(), group(row), ciphertext, peerKey(row), peerCredential(row), sessionId, eventId, sender, ownId())
        val journal = JSONObject().put("v", 1).put("client_message_id", eventId).put("conversation_id", threadId(row)).put("from_void_id", sender).put("to_void_id", ownId())
          .put("body", decrypted.body).put("created_at", decrypted.createdAt.toLong()).toString().toByteArray(Charsets.UTF_8)
        try { storage.journal("decrypt", group(row), journal, ciphertext, peerCredential(row), peerKey(row), checkedGroup(engine, row).epoch.toString()) } finally { journal.fill(0) }
        journalId = one("SELECT id FROM crypto_events ORDER BY rowid DESC LIMIT 1")!!.getString("id")
        if (row.getString("status") == "awaiting_ack") activate(row, engine)
        else db.execSQL("UPDATE messaging_sessions SET last_error=NULL WHERE id=?", arrayOf(sessionId))
        outcome = "staged"
      } catch (error: MlsException) {
        if (error is MlsException.StorageFailure || error is MlsException.InvalidState) throw error
        diagnostic("decrypt_application", error)
        rejectedCode = "MLS_" + error.javaClass.simpleName.uppercase()
      } catch (_: IllegalArgumentException) { rejectedCode = "INVALID_APPLICATION" }
      catch (_: org.json.JSONException) { rejectedCode = "INVALID_APPLICATION" }
      catch (error: StorageFailure) {
        if (error.code !in setOf("CONFLICT", "NOT_FOUND", "INVALID_INPUT", "PEER_KEY_CHANGED")) throw error
        rejectedCode = error.code
      }
      db.execSQL("INSERT INTO messaging_inbox(sender_void_id,event_id,payload_hash,session_id,journal_id,outcome,processed_at) VALUES(?,?,?,?,?,?,?)", arrayOf(sender, eventId, digest, sessionId, journalId, outcome, now()))
      if (rejectedCode != null && sessionId != null) db.execSQL("UPDATE messaging_sessions SET last_error=? WHERE id=? AND peer_void_id=?", arrayOf(rejectedCode, sessionId, sender))
    }
    recoverInbox()
    resumePending()
    val saved = one("SELECT outcome FROM messaging_inbox WHERE sender_void_id=? AND event_id=?", sender, eventId)!!
    return JSONObject().put("sender_void_id", sender).put("event_id", eventId).put("changed", saved.getString("outcome") == "applied")
  }
  /** Deliberately separate from the receiving ratchet transaction for crash recovery. */
  private fun recoverInbox() {
    val pending = rows("SELECT * FROM messaging_inbox WHERE outcome='staged' ORDER BY processed_at,event_id")
    for (index in 0 until pending.length()) {
      val row = pending.getJSONObject(index)
      transaction {
        val bytes = db.rawQuery("SELECT plaintext FROM crypto_events WHERE id=? AND operation='decrypt' AND outcome='ok'", arrayOf(row.getString("journal_id"))).use {
          if (!it.moveToFirst() || it.isNull(0)) throw StorageFailure("DATABASE_ERROR")
          it.getBlob(0)
        }
        val plain = try { json(bytes.toString(Charsets.UTF_8)) } finally { bytes.fill(0) }
        require(plain.getString("client_message_id") == row.getString("event_id") && plain.getString("conversation_id") == threadId(wireSession(row.getString("session_id"))) && plain.getString("from_void_id") == row.getString("sender_void_id") && plain.getString("to_void_id") == ownId())
        PublicRepositories(db).execute("messages.save", JSONObject().put("id", plain.getString("client_message_id")).put("conversationId", plain.getString("conversation_id"))
          .put("senderVoidId", plain.getString("from_void_id")).put("body", plain.getString("body")).put("direction", "incoming").put("deliveryState", "received").put("createdAt", plain.getLong("created_at")))
        db.execSQL("UPDATE messaging_inbox SET outcome='applied' WHERE sender_void_id=? AND event_id=? AND outcome='staged'", arrayOf(row.getString("sender_void_id"), row.getString("event_id")))
      }
    }
  }
}
