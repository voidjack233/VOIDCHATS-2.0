package com.voidchats.storage

import android.database.Cursor
import android.database.sqlite.SQLiteConstraintException
import net.zetetic.database.sqlcipher.SQLiteDatabase
import org.json.JSONArray
import org.json.JSONObject

/** Only these named application operations exist. No SQL or private tables enter from JS. */
internal class PublicRepositories(private val db: SQLiteDatabase) {
  fun execute(operation: String, p: JSONObject): Any? = try {
    when (operation) {
      "conversations.list" -> { fields(p); array("SELECT id FROM conversations ORDER BY updated_at DESC,id DESC") { conversation(it.getString(0))!! } }
      "conversations.page" -> conversationPage(p)
      "conversations.get" -> { fields(p, "id"); conversation(text(p, "id")) }
      "conversations.save" -> saveConversation(p)
      "conversations.markRead" -> markRead(p)
      "messages.page" -> page(p)
      "messages.get" -> { fields(p, "id"); message(text(p, "id")) }
      "messages.save" -> saveMessage(p)
      "messages.edit" -> {
        fields(p, "id", "body", "editedAt"); val id = text(p, "id"); val previous = requireMessage(id)
        require(previous.isNull("deletedAt")); val at = time(p, "editedAt"); require(at >= previous.getLong("createdAt"))
        db.execSQL("UPDATE messages SET body=?,edited_at=? WHERE id=?", arrayOf(text(p, "body", 262144, blank = true), at, id)); message(id)
      }
      "messages.delete" -> {
        fields(p, "id", "deletedAt"); val id = text(p, "id"); val previous = requireMessage(id); val at = time(p, "deletedAt")
        require(at >= previous.getLong("createdAt"))
        if (previous.isNull("deletedAt")) db.execSQL("UPDATE messages SET body=NULL,deleted_at=? WHERE id=?", arrayOf(at, id))
        message(id)
      }
      "messages.setDeliveryState" -> {
        fields(p, "id", "state"); val id = text(p, "id"); requireMessage(id)
        val state = choice(p, "state", "pending", "sent", "delivered", "read", "failed")
        db.execSQL("UPDATE messages SET delivery_state=? WHERE id=?", arrayOf(state, id)); message(id)
      }
      "messages.setReaction" -> reaction(p)
      "messages.setReceipt" -> receipt(p)
      "contacts.list" -> { fields(p); array("SELECT id FROM contacts ORDER BY display_name COLLATE NOCASE,id") { contact(it.getString(0))!! } }
      "contacts.get" -> { fields(p, "id"); contact(text(p, "id")) }
      "contacts.save" -> saveContact(p)
      "contacts.remove" -> { fields(p, "id"); db.execSQL("DELETE FROM contacts WHERE id=?", arrayOf(text(p, "id"))); null }
      "requests.list" -> {
        fields(p, "status"); val status = nullableText(p, "status")
        require(status == null || status in listOf("pending", "accepted", "declined", "blocked"))
        if (status == null) array("SELECT id FROM message_requests ORDER BY created_at DESC,id DESC") { request(it.getString(0))!! }
        else array("SELECT id FROM message_requests WHERE status=? ORDER BY created_at DESC,id DESC", arrayOf(status)) { request(it.getString(0))!! }
      }
      "requests.save" -> saveRequest(p)
      "requests.setStatus" -> {
        fields(p, "id", "status", "updatedAt"); val id = text(p, "id"); val previous = request(id) ?: throw StorageFailure("NOT_FOUND")
        val at = time(p, "updatedAt"); require(at >= previous.getLong("updatedAt"))
        db.execSQL("UPDATE message_requests SET status=?,updated_at=? WHERE id=?", arrayOf(choice(p, "status", "pending", "accepted", "declined", "blocked"), at, id)); request(id)
      }
      else -> throw IllegalArgumentException()
    }
  } catch (_: SQLiteConstraintException) { throw StorageFailure("CONFLICT") }

  private fun saveConversation(p: JSONObject): JSONObject {
    fields(p, "id", "kind", "title", "peerVoidId", "mlsGroupId", "createdAt", "updatedAt", optional = setOf("members"))
    val id = text(p, "id"); val kind = choice(p, "kind", "direct", "group"); val peer = nullableText(p, "peerVoidId")
    require((kind == "direct" && peer != null) || (kind == "group" && peer == null))
    val created = time(p, "createdAt"); val updated = time(p, "updatedAt"); require(updated >= created)
    val previous = conversation(id)
    if (previous != null) {
      require(previous.getString("kind") == kind && previous.getLong("createdAt") == created && previous.opt("peerVoidId") == (peer ?: JSONObject.NULL))
      require(updated >= previous.getLong("updatedAt"))
    }
    db.execSQL("INSERT INTO conversations(id,kind,title,peer_void_id,mls_group_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET title=excluded.title,mls_group_id=excluded.mls_group_id,updated_at=excluded.updated_at",
      arrayOf(id, kind, text(p, "title", 4096), peer, nullableText(p, "mlsGroupId", 4096), created, updated))
    if (p.has("members")) {
      val members = p.get("members") as? JSONArray ?: throw IllegalArgumentException()
      require(members.length() <= 1000)
      db.execSQL("DELETE FROM conversation_members WHERE conversation_id=?", arrayOf(id))
      for (i in 0 until members.length()) {
        val member = members.get(i) as? JSONObject ?: throw IllegalArgumentException()
        fields(member, "voidId", "displayName", "role", "joinedAt", "leftAt")
        val joined = time(member, "joinedAt"); val left = nullableTime(member, "leftAt"); require(left == null || left >= joined)
        db.execSQL("INSERT INTO conversation_members(conversation_id,void_id,display_name,role,joined_at,left_at) VALUES(?,?,?,?,?,?)",
          arrayOf(id, text(member, "voidId"), text(member, "displayName", 4096, blank = true), choice(member, "role", "member", "admin"), joined, left))
      }
    }
    return conversation(id)!!
  }

  private fun conversationPage(p: JSONObject): JSONObject {
    fields(p, "limit", "before")
    val requestedLimit = time(p, "limit"); require(requestedLimit in 1L..100L)
    val limit = requestedLimit.toInt()
    val before = if (p.isNull("before")) null else p.get("before") as? JSONObject ?: throw IllegalArgumentException()
    if (before != null) fields(before, "updatedAt", "id")
    val beforeAt = before?.let { time(it, "updatedAt") }; val beforeId = before?.let { text(it, "id") }
    val items = if (before == null) array("SELECT id FROM conversations ORDER BY updated_at DESC,id DESC LIMIT ?", arrayOf((limit + 1).toString())) { conversation(it.getString(0))!! }
      else array("SELECT id FROM conversations WHERE updated_at<? OR (updated_at=? AND id<?) ORDER BY updated_at DESC,id DESC LIMIT ?", arrayOf(beforeAt.toString(), beforeAt.toString(), beforeId, (limit + 1).toString())) { conversation(it.getString(0))!! }
    val more = items.length() > limit
    if (more) items.remove(items.length() - 1)
    val last = if (more && items.length() > 0) items.getJSONObject(items.length() - 1) else null
    return JSONObject().put("items", items).put("nextCursor", if (last == null) JSONObject.NULL else JSONObject().put("updatedAt", last.getLong("updatedAt")).put("id", last.getString("id")))
  }

  private fun conversation(id: String): JSONObject? = single("SELECT id,kind,title,peer_void_id,mls_group_id,created_at,updated_at FROM conversations WHERE id=?", arrayOf(id)) { c ->
    JSONObject().apply {
      put("id", c.getString(0)); put("kind", c.getString(1)); put("title", c.getString(2)); put("peerVoidId", nullable(c, 3)); put("mlsGroupId", nullable(c, 4))
      put("createdAt", c.getLong(5)); put("updatedAt", c.getLong(6))
      put("unreadCount", scalar("SELECT count(*) FROM messages WHERE conversation_id=? AND direction='incoming' AND deleted_at IS NULL AND read_at IS NULL", arrayOf(id)))
      put("lastMessage", single("SELECT id,body,created_at,deleted_at FROM messages WHERE conversation_id=? ORDER BY created_at DESC,id DESC LIMIT 1", arrayOf(id)) {
        JSONObject().put("id", it.getString(0)).put("body", nullable(it, 1)).put("createdAt", it.getLong(2)).put("deletedAt", nullable(it, 3, number = true))
      } ?: JSONObject.NULL)
      put("members", array("SELECT void_id,display_name,role,joined_at,left_at FROM conversation_members WHERE conversation_id=? ORDER BY joined_at,void_id", arrayOf(id)) {
        JSONObject().put("voidId", it.getString(0)).put("displayName", it.getString(1)).put("role", it.getString(2)).put("joinedAt", it.getLong(3)).put("leftAt", nullable(it, 4, number = true))
      })
    }
  }

  private fun markRead(p: JSONObject): Any? {
    fields(p, "conversationId", "through", "readAt")
    val conversationId = text(p, "conversationId"); conversation(conversationId) ?: throw StorageFailure("NOT_FOUND")
    val cursor = p.get("through") as? JSONObject ?: throw IllegalArgumentException(); fields(cursor, "id", "createdAt")
    val id = text(cursor, "id"); val at = time(cursor, "createdAt"); val readAt = time(p, "readAt")
    val through = requireMessage(id); require(through.getString("conversationId") == conversationId && through.getLong("createdAt") == at)
    db.execSQL("UPDATE messages SET read_at=? WHERE conversation_id=? AND direction='incoming' AND deleted_at IS NULL AND read_at IS NULL AND (created_at<? OR (created_at=? AND id<=?))", arrayOf(readAt, conversationId, at, at, id))
    db.execSQL("UPDATE conversations SET local_read_at=?,local_read_message_id=? WHERE id=? AND (local_read_at IS NULL OR local_read_at<? OR (local_read_at=? AND local_read_message_id<?))", arrayOf(at, id, conversationId, at, at, id))
    return null
  }

  private fun saveMessage(p: JSONObject): JSONObject {
    fields(p, "id", "conversationId", "senderVoidId", "body", "direction", "deliveryState", "createdAt")
    val id = text(p, "id"); val conversationId = text(p, "conversationId"); val sender = text(p, "senderVoidId")
    val direction = choice(p, "direction", "incoming", "outgoing"); val delivery = choice(p, "deliveryState", "received", "pending")
    require((direction == "incoming" && delivery == "received") || (direction == "outgoing" && delivery == "pending"))
    val body = text(p, "body", 262144, blank = true); val created = time(p, "createdAt")
    val previous = message(id)
    if (previous != null) {
      // Duplicate deliveries cannot reset edits, receipts, read state or tombstones.
      if (previous.getString("conversationId") != conversationId || previous.getString("senderVoidId") != sender || previous.getString("direction") != direction || previous.getLong("createdAt") != created) throw StorageFailure("CONFLICT")
      return previous
    }
    conversation(conversationId) ?: throw StorageFailure("NOT_FOUND")
    db.execSQL("INSERT INTO messages(id,conversation_id,sender_void_id,body,direction,delivery_state,created_at) VALUES(?,?,?,?,?,?,?)", arrayOf(id, conversationId, sender, body, direction, delivery, created))
    db.execSQL("UPDATE conversations SET updated_at=MAX(updated_at,?) WHERE id=?", arrayOf(created, conversationId))
    return message(id)!!
  }

  private fun message(id: String): JSONObject? = single("SELECT id,conversation_id,sender_void_id,body,direction,delivery_state,created_at,edited_at,deleted_at,read_at FROM messages WHERE id=?", arrayOf(id)) { c ->
    JSONObject().apply {
      put("id", c.getString(0)); put("conversationId", c.getString(1)); put("senderVoidId", c.getString(2)); put("body", nullable(c, 3)); put("direction", c.getString(4)); put("deliveryState", c.getString(5))
      put("createdAt", c.getLong(6)); put("editedAt", nullable(c, 7, true)); put("deletedAt", nullable(c, 8, true)); put("readAt", nullable(c, 9, true))
      put("reactions", array("SELECT reactor_void_id,emoji,created_at FROM message_reactions WHERE message_id=? ORDER BY created_at,reactor_void_id,emoji", arrayOf(id)) {
        JSONObject().put("reactorVoidId", it.getString(0)).put("emoji", it.getString(1)).put("createdAt", it.getLong(2))
      })
      put("receipts", array("SELECT recipient_void_id,state,recorded_at FROM message_receipts WHERE message_id=? ORDER BY recipient_void_id", arrayOf(id)) {
        JSONObject().put("recipientVoidId", it.getString(0)).put("state", it.getString(1)).put("recordedAt", it.getLong(2))
      })
    }
  }
  private fun requireMessage(id: String) = message(id) ?: throw StorageFailure("NOT_FOUND")

  private fun page(p: JSONObject): JSONObject {
    fields(p, "conversationId", "limit", "before")
    val id = text(p, "conversationId"); val requestedLimit = time(p, "limit"); require(requestedLimit in 1L..100L); val limit = requestedLimit.toInt()
    val before = if (p.isNull("before")) null else p.get("before") as? JSONObject ?: throw IllegalArgumentException()
    if (before != null) fields(before, "id", "createdAt")
    val beforeAt = before?.let { time(it, "createdAt") }; val beforeId = before?.let { text(it, "id") }
    val result = if (before == null) array("SELECT id FROM messages WHERE conversation_id=? ORDER BY created_at DESC,id DESC LIMIT ?", arrayOf(id, (limit + 1).toString())) { message(it.getString(0))!! }
      else array("SELECT id FROM messages WHERE conversation_id=? AND (created_at<? OR (created_at=? AND id<?)) ORDER BY created_at DESC,id DESC LIMIT ?",
        arrayOf(id, beforeAt.toString(), beforeAt.toString(), beforeId, (limit + 1).toString())) { message(it.getString(0))!! }
    val more = result.length() > limit
    if (more) result.remove(result.length() - 1)
    val last = if (more && result.length() > 0) result.getJSONObject(result.length() - 1) else null
    return JSONObject().put("items", result).put("nextCursor", if (last != null) JSONObject().put("createdAt", last.getLong("createdAt")).put("id", last.getString("id")) else JSONObject.NULL)
  }

  private fun reaction(p: JSONObject): JSONObject {
    fields(p, "id", "reactorVoidId", "emoji", "active", "createdAt")
    val id = text(p, "id"); val prior = requireMessage(id); require(prior.isNull("deletedAt"))
    val reactor = text(p, "reactorVoidId"); val emoji = text(p, "emoji", 128); val active = p.get("active") as? Boolean ?: throw IllegalArgumentException(); val at = time(p, "createdAt")
    if (active) db.execSQL("INSERT INTO message_reactions(message_id,reactor_void_id,emoji,created_at) VALUES(?,?,?,?) ON CONFLICT(message_id,reactor_void_id,emoji) DO NOTHING", arrayOf(id, reactor, emoji, at))
    else db.execSQL("DELETE FROM message_reactions WHERE message_id=? AND reactor_void_id=? AND emoji=?", arrayOf(id, reactor, emoji))
    return message(id)!!
  }

  private fun receipt(p: JSONObject): JSONObject {
    fields(p, "id", "recipientVoidId", "state", "recordedAt")
    val id = text(p, "id"); requireMessage(id); val recipient = text(p, "recipientVoidId"); val state = choice(p, "state", "delivered", "read"); val at = time(p, "recordedAt")
    db.execSQL("INSERT INTO message_receipts(message_id,recipient_void_id,state,recorded_at) VALUES(?,?,?,?) ON CONFLICT(message_id,recipient_void_id) DO UPDATE SET state=CASE WHEN message_receipts.state='read' THEN 'read' ELSE excluded.state END,recorded_at=MAX(message_receipts.recorded_at,excluded.recorded_at)", arrayOf(id, recipient, state, at))
    return message(id)!!
  }

  private fun contact(id: String): JSONObject? = single("SELECT id,void_id,display_name,identity_public_key,trust_state,created_at,updated_at FROM contacts WHERE id=?", arrayOf(id)) {
    JSONObject().put("id", it.getString(0)).put("voidId", it.getString(1)).put("displayName", it.getString(2)).put("identityPublicKey", it.getString(3)).put("trustState", it.getString(4)).put("createdAt", it.getLong(5)).put("updatedAt", it.getLong(6))
  }

  private fun saveContact(p: JSONObject): JSONObject {
    fields(p, "id", "voidId", "displayName", "identityPublicKey", "createdAt", "updatedAt")
    val id = text(p, "id"); val voidId = text(p, "voidId"); val key = text(p, "identityPublicKey", 65536); val created = time(p, "createdAt"); val updated = time(p, "updatedAt"); require(updated >= created)
    val previous = contact(id)
    if (previous != null) require(previous.getString("voidId") == voidId && previous.getLong("createdAt") == created && updated >= previous.getLong("updatedAt"))
    db.execSQL("INSERT INTO contacts(id,void_id,display_name,identity_public_key,trust_state,created_at,updated_at) VALUES(?,?,?,?,'unverified',?,?) ON CONFLICT(id) DO UPDATE SET display_name=excluded.display_name,trust_state=CASE WHEN contacts.identity_public_key<>excluded.identity_public_key THEN 'changed' ELSE contacts.trust_state END,identity_public_key=excluded.identity_public_key,updated_at=excluded.updated_at",
      arrayOf(id, voidId, text(p, "displayName", 4096), key, created, updated))
    return contact(id)!!
  }

  private fun request(id: String): JSONObject? = single("SELECT id,conversation_id,from_void_id,display_name,identity_public_key,preview,status,created_at,updated_at FROM message_requests WHERE id=?", arrayOf(id)) {
    JSONObject().put("id", it.getString(0)).put("conversationId", nullable(it, 1)).put("fromVoidId", it.getString(2)).put("displayName", it.getString(3)).put("identityPublicKey", it.getString(4)).put("preview", it.getString(5)).put("status", it.getString(6)).put("createdAt", it.getLong(7)).put("updatedAt", it.getLong(8))
  }

  private fun saveRequest(p: JSONObject): JSONObject {
    fields(p, "id", "conversationId", "fromVoidId", "displayName", "identityPublicKey", "preview", "createdAt")
    val id = text(p, "id"); val previous = request(id); val from = text(p, "fromVoidId"); val created = time(p, "createdAt")
    if (previous != null) { if (previous.getString("fromVoidId") != from || previous.getLong("createdAt") != created) throw StorageFailure("CONFLICT"); return previous }
    db.execSQL("INSERT INTO message_requests(id,conversation_id,from_void_id,display_name,identity_public_key,preview,status,created_at,updated_at) VALUES(?,?,?,?,?,?,'pending',?,?)",
      arrayOf(id, nullableText(p, "conversationId"), from, text(p, "displayName", 4096, true), text(p, "identityPublicKey", 65536), text(p, "preview", 262144, true), created, created))
    return request(id)!!
  }

  private fun fields(p: JSONObject, vararg required: String, optional: Set<String> = emptySet()) {
    val keys = p.keys().asSequence().toSet(); require(keys.containsAll(required.toSet()) && keys.all { it in required || it in optional })
  }
  private fun text(p: JSONObject, name: String, max: Int = 512, blank: Boolean = false): String {
    val value = p.get(name) as? String ?: throw IllegalArgumentException()
    require(value.length <= max && (blank || value.isNotBlank()) && '\u0000' !in value && value.toByteArray(Charsets.UTF_8).toString(Charsets.UTF_8) == value)
    return value
  }
  private fun nullableText(p: JSONObject, name: String, max: Int = 512): String? = if (p.isNull(name)) null else text(p, name, max)
  private fun time(p: JSONObject, name: String): Long {
    val value = p.get(name) as? Number ?: throw IllegalArgumentException(); val long = value.toLong()
    require(long >= 0 && long <= 9_007_199_254_740_991L && value.toDouble() == long.toDouble()); return long
  }
  private fun nullableTime(p: JSONObject, name: String): Long? = if (p.isNull(name)) null else time(p, name)
  private fun choice(p: JSONObject, name: String, vararg choices: String): String = text(p, name).also { require(it in choices) }
  private fun nullable(c: Cursor, column: Int, number: Boolean = false): Any = if (c.isNull(column)) JSONObject.NULL else if (number) c.getLong(column) else c.getString(column)
  private fun scalar(sql: String, parameters: Array<String?>): Long = db.rawQuery(sql, parameters).use { require(it.moveToFirst()); it.getLong(0) }
  private fun <T> single(sql: String, parameters: Array<String?>, transform: (Cursor) -> T): T? = db.rawQuery(sql, parameters).use { if (it.moveToFirst()) transform(it) else null }
  private fun array(sql: String, parameters: Array<String?> = emptyArray(), transform: (Cursor) -> Any): JSONArray = db.rawQuery(sql, parameters).use {
    JSONArray().apply { while (it.moveToNext()) put(transform(it)) }
  }
}
