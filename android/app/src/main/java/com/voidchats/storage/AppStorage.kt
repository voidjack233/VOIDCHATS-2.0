package com.voidchats.storage

import android.content.Context
import android.util.Base64
import com.facebook.react.bridge.Promise
import com.voidchats.BuildConfig
import com.voidchats.openmls.MlsEngine
import com.voidchats.openmls.MlsException
import com.voidchats.openmls.MlsStorage
import com.voidchats.openmls.StoredMlsState
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import net.zetetic.database.sqlcipher.SQLiteDatabase
import org.json.JSONObject
import org.json.JSONArray
import org.json.JSONTokener

/** One queue and one transaction boundary for public history and private MLS checkpoints. */
internal class AppStorage(private val context: Context, namespace: String = "void-local-v1") {
  private val encrypted = EncryptedDatabase(context, namespace)
  private var database: SQLiteDatabase? = null
  private var engine: MlsEngine? = null
  private var errorCode: String? = null
  private var checkpointed = false
  private val loadedSnapshotBuffers = mutableListOf<ByteArray>()
  private var transactionActive = false
  private var references = 2
  private val executor = object : ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(32)) {
    override fun terminated() { closeHandles(); super.terminated() }
  }

  fun submit(promise: Promise, operation: () -> Any?) {
    try {
      executor.execute {
        try { promise.resolve(operation()) }
        catch (error: StorageFailure) {
          if (error.code == "DATABASE_ERROR") poison("DATABASE_ERROR")
          promise.reject(error.code.removePrefix("STORAGE_"), "Protected local storage is unavailable.")
        }
        catch (_: IllegalArgumentException) { promise.reject("INVALID_INPUT", "The local storage input is invalid.") }
        catch (error: MlsException) {
          if (error is MlsException.StorageFailure) poison("DATABASE_ERROR")
          val category = error.javaClass.simpleName.replace(Regex("([a-z])([A-Z])"), "$1_$2").uppercase()
          promise.reject("MLS_$category", "The native MLS operation failed.")
        }
        catch (_: LinkageError) { poison("NATIVE_MODULE_UNAVAILABLE"); promise.reject("NATIVE_MODULE_UNAVAILABLE", "The protected native library could not be loaded.") }
        catch (_: Exception) { poison("DATABASE_ERROR"); promise.reject("DATABASE_ERROR", "Protected local storage is unavailable.") }
      }
    } catch (_: RejectedExecutionException) { promise.reject("OPERATION_FAILED", "The protected native queue cannot accept another operation.") }
  }

  @Synchronized fun releaseModule() {
    references--
    if (references <= 0) executor.shutdown()
  }

  fun status(): String = JSONObject().apply {
    val currentStatus = if (errorCode != null) "error" else if (database != null) "open" else if (encrypted.exists()) "locked" else "not_initialized"
    put("status", currentStatus)
    put("schemaVersion", database?.version ?: JSONObject.NULL)
    put("identity", if (database != null) localIdentity() ?: JSONObject.NULL else JSONObject.NULL)
    put("errorCode", errorCode ?: JSONObject.NULL)
  }.toString()

  fun open(): String = encrypted.withLifecycleLock {
    if (database != null) return@withLifecycleLock status()
    try {
      if (!encrypted.exists()) { errorCode = null; return@withLifecycleLock status() }
      val db = encrypted.open()
      StorageMigrations(context).migrate(db)
      database = db
      val identity = localIdentity()
      if (identity == null) throw StorageFailure("DATABASE_ERROR")
      engine = openEngine(false)
      val clients = engine!!.listClients()
      val primary = clients.singleOrNull { it.clientHandle == identity.getString("clientHandle") } ?: throw StorageFailure("DATABASE_ERROR")
      if (base64(primary.credentialIdentity) != identity.getString("credentialIdentityBase64") ||
          base64(primary.signaturePublicKey) != identity.getString("signaturePublicKeyBase64")) throw StorageFailure("DATABASE_ERROR")
      errorCode = null
      return@withLifecycleLock status()
    } catch (error: Throwable) {
      val code = if (error is StorageFailure) error.code.removePrefix("STORAGE_") else "DATABASE_ERROR"
      poison(code)
      throw StorageFailure(code)
    }
  }

  fun initializeDevelopment(): String = encrypted.withLifecycleLock {
    if (!BuildConfig.DEBUG) throw StorageFailure("DEVELOPMENT_ONLY")
    if (encrypted.exists()) return@withLifecycleLock open()
    try {
      val db = encrypted.open(create = true)
      database = db
      StorageMigrations(context).migrate(db)
      mutateMls(initializeIfMissing = true) { active ->
        val localId = UUID.randomUUID().toString()
        val client = active.createClient(localId.toByteArray(Charsets.UTF_8))
        db.execSQL("INSERT INTO local_identity(slot,local_device_id,client_handle,credential_identity_base64,signature_public_key_base64,account_void_id,created_at) VALUES(1,?,?,?,?,NULL,?)",
          arrayOf(localId, client.clientHandle, base64(client.credentialIdentity), base64(client.signaturePublicKey), System.currentTimeMillis()))
      }
      errorCode = null
      return@withLifecycleLock status()
    } catch (error: Throwable) { poison("DATABASE_ERROR"); throw StorageFailure("DATABASE_ERROR") }
  }

  fun localIdentity(): JSONObject? {
    val db = requireDatabase()
    return db.rawQuery("SELECT local_device_id,client_handle,credential_identity_base64,signature_public_key_base64,account_void_id,created_at FROM local_identity WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst()) null else JSONObject().apply {
        put("localDeviceId", it.getString(0)); put("clientHandle", it.getString(1))
        put("credentialIdentityBase64", it.getString(2)); put("signaturePublicKeyBase64", it.getString(3))
        put("accountVoidId", if (it.isNull(4)) JSONObject.NULL else it.getString(4)); put("createdAt", it.getLong(5))
      }
    }
  }

  fun requireDatabase(): SQLiteDatabase = database ?: throw StorageFailure(if (encrypted.exists()) "LOCKED" else "NOT_INITIALIZED")
  fun requireEngine(): MlsEngine = engine ?: throw StorageFailure("LOCKED")
  fun requirePrimary(handle: String) {
    require(handle == localIdentity()?.getString("clientHandle"))
  }

  private val callback = object : MlsStorage {
    override fun loadState(): StoredMlsState? = loadMlsState()?.also { loadedSnapshotBuffers.add(it.snapshot) }
    override fun stateRevision(): ULong = requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst()) 0uL else it.getLong(0).toULong()
    }
    override fun saveState(expectedRevision: ULong, snapshot: ByteArray): ULong {
      try {
        if (!transactionActive || !requireDatabase().inTransaction() || expectedRevision >= Long.MAX_VALUE.toULong() || snapshot.isEmpty() || snapshot.size > 64 * 1024 * 1024) throw MlsException.StorageFailure()
        val next = expectedRevision + 1uL
        val db = requireDatabase()
        if (expectedRevision == 0uL) {
          db.compileStatement("INSERT INTO native_mls_state(slot,revision,snapshot) VALUES(1,1,?)").use {
            it.bindBlob(1, snapshot); it.executeInsert()
          }
        } else {
          db.compileStatement("UPDATE native_mls_state SET revision=?,snapshot=? WHERE slot=1 AND revision=?").use {
            it.bindLong(1, next.toLong()); it.bindBlob(2, snapshot); it.bindLong(3, expectedRevision.toLong())
            if (it.executeUpdateDelete() != 1) throw MlsException.StorageFailure()
          }
        }
        checkpointed = true
        return next
      } catch (_: Exception) { throw MlsException.StorageFailure() }
      finally { snapshot.fill(0) }
    }
  }

  private fun openEngine(initializeIfMissing: Boolean): MlsEngine = try {
    MlsEngine.open(callback, initializeIfMissing)
  } finally {
    // UniFFI synchronously lifted each callback result into Rust before returning.
    for (snapshot in loadedSnapshotBuffers) snapshot.fill(0)
    loadedSnapshotBuffers.clear()
  }

  // CursorWindow has a bounded row size. Read large private snapshots in memory chunks.
  internal fun loadMlsState(): StoredMlsState? {
    val db = requireDatabase()
    val metadata = db.rawQuery("SELECT revision,length(snapshot) FROM native_mls_state WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst()) null else Pair(it.getLong(0), it.getInt(1))
    } ?: return null
    if (metadata.first < 1 || metadata.second !in 1..(64 * 1024 * 1024)) throw MlsException.StorageFailure()
    val snapshot = ByteArray(metadata.second)
    try {
      var offset = 0
      while (offset < snapshot.size) {
        val length = minOf(256 * 1024, snapshot.size - offset)
        db.rawQuery("SELECT substr(snapshot,?,?) FROM native_mls_state WHERE slot=1 AND revision=?", arrayOf((offset + 1).toString(), length.toString(), metadata.first.toString())).use {
          if (!it.moveToFirst()) throw MlsException.StorageFailure()
          val chunk = it.getBlob(0)
          try { if (chunk.size != length) throw MlsException.StorageFailure(); chunk.copyInto(snapshot, offset) } finally { chunk.fill(0) }
        }
        offset += length
      }
      return StoredMlsState(metadata.first.toULong(), snapshot)
    } catch (_: Exception) { snapshot.fill(0); throw MlsException.StorageFailure() }
  }

  fun readStoredDecryptions(beforeJSON: String, limit: Double): String {
    require(beforeJSON.length <= 1024 && limit.isFinite() && limit == limit.toInt().toDouble() && limit in 1.0..100.0)
    val parser = JSONTokener(beforeJSON)
    val parsed = parser.nextValue(); require(parser.nextClean() == '\u0000')
    val before = if (parsed == JSONObject.NULL) null else parsed as? JSONObject ?: throw IllegalArgumentException()
    val beforeId: String?; val beforeAt: Long?
    if (before != null) {
      require(before.keys().asSequence().toSet() == setOf("createdAt", "id"))
      beforeId = before.get("id") as? String ?: throw IllegalArgumentException()
      val number = before.get("createdAt") as? Number ?: throw IllegalArgumentException(); beforeAt = number.toLong()
      require(beforeId.isNotBlank() && beforeId.length <= 128 && beforeAt in 0..9_007_199_254_740_991L && number.toDouble() == beforeAt.toDouble())
    } else { beforeId = null; beforeAt = null }
    val localId = localIdentity()?.getString("localDeviceId") ?: throw StorageFailure("NOT_INITIALIZED")
    val db = requireDatabase()
    db.rawQuery("SELECT count(*) FROM crypto_events WHERE local_id=? AND operation='decrypt' AND outcome='ok' AND (group_id IS NULL OR plaintext IS NULL OR sender_identity IS NULL OR sender_signature_public_key IS NULL OR epoch IS NULL OR checkpoint_revision IS NULL)", arrayOf(localId)).use {
      if (!it.moveToFirst() || it.getLong(0) != 0L) throw StorageFailure("DATABASE_ERROR")
    }
    val columns = "SELECT id,group_id,plaintext,sender_identity,sender_signature_public_key,epoch,created_at FROM crypto_events WHERE local_id=? AND operation='decrypt' AND outcome='ok'"
    val sql = if (before == null) "$columns ORDER BY created_at DESC,id DESC LIMIT ?" else "$columns AND (created_at<? OR (created_at=? AND id<?)) ORDER BY created_at DESC,id DESC LIMIT ?"
    val args = if (before == null) arrayOf(localId, (limit.toInt() + 1).toString()) else arrayOf(localId, beforeAt.toString(), beforeAt.toString(), beforeId!!, (limit.toInt() + 1).toString())
    val items = JSONArray(); var more = false; var encodedSize = 0
    db.rawQuery(sql, args).use { c ->
      while (c.moveToNext()) {
        if (items.length() >= limit.toInt()) { more = true; break }
        val plaintext = c.getBlob(2)
        try {
          val groupId = c.getBlob(1); val senderIdentity = c.getBlob(3); val senderKey = c.getBlob(4)
          val eventId = c.getString(0); val epoch = c.getString(5); val createdAt = c.getLong(6)
          if (plaintext.size > 1024 * 1024 || groupId.size !in 1..256 || senderIdentity.size !in 1..1024 || senderKey.size != 32 ||
              eventId.isBlank() || eventId.length > 128 || createdAt !in 0..9_007_199_254_740_991L ||
              !epoch.matches(Regex("0|[1-9][0-9]{0,19}")) || (epoch.length == 20 && epoch > "18446744073709551615")) throw StorageFailure("DATABASE_ERROR")
          val item = JSONObject().put("eventId", eventId).put("groupIdBase64", base64(groupId)).put("plaintextBase64", base64(plaintext))
            .put("senderIdentityBase64", base64(senderIdentity)).put("senderSignaturePublicKeyBase64", base64(senderKey)).put("epoch", epoch).put("createdAt", createdAt)
          val size = item.toString().length + 1
          if (items.length() > 0 && encodedSize + size > 4 * 1024 * 1024 - 1024) { more = true; break }
          items.put(item); encodedSize += size
        } finally { plaintext.fill(0) }
      }
    }
    val last = if (more && items.length() > 0) items.getJSONObject(items.length() - 1) else null
    return JSONObject().put("items", items).put("nextCursor", if (last == null) JSONObject.NULL else JSONObject().put("createdAt", last.getLong("createdAt")).put("id", last.getString("eventId"))).toString()
  }

  fun <T> mutateMls(initializeIfMissing: Boolean = false, operation: (MlsEngine) -> T): T {
    val db = requireDatabase()
    db.beginTransaction()
    checkpointed = false
    transactionActive = true
    var failure: Throwable? = null
    var result: T? = null
    try {
      val active = engine ?: openEngine(initializeIfMissing).also { engine = it }
      try { result = operation(active) }
      catch (error: Throwable) { failure = error }
      // Rust can consume a ratchet/key package before returning a protocol error.
      // A successful checkpoint must survive even when the operation is rejected.
      if (failure == null || (checkpointed && failure is MlsException && failure !is MlsException.StorageFailure)) {
        db.setTransactionSuccessful()
      } else if (failure is MlsException.StorageFailure || failure !is MlsException) {
        engine?.poison()
      }
    } catch (error: Throwable) { failure = error; engine?.poison() }
    finally {
      transactionActive = false
      try { db.endTransaction() }
      catch (error: Throwable) { failure = StorageFailure("DATABASE_ERROR") }
    }
    val error = failure
    if (error != null) {
      if (error !is MlsException || error is MlsException.StorageFailure) poison("DATABASE_ERROR")
      throw error
    }
    @Suppress("UNCHECKED_CAST") return result as T
  }

  fun journal(operation: String, groupId: ByteArray?, plaintext: ByteArray?, ciphertext: ByteArray?, senderIdentity: ByteArray? = null, senderSignaturePublicKey: ByteArray? = null, epoch: String? = null) {
    check(transactionActive)
    requireDatabase().execSQL("INSERT INTO crypto_events(id,local_id,operation,group_id,plaintext,ciphertext,outcome,created_at,sender_identity,sender_signature_public_key,epoch,checkpoint_revision) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
      arrayOf(UUID.randomUUID().toString(), localIdentity()!!.getString("localDeviceId"), operation, groupId, plaintext, ciphertext, "ok", System.currentTimeMillis(), senderIdentity, senderSignaturePublicKey, epoch, callback.stateRevision().toLong()))
  }

  fun execute(operation: String, payload: JSONObject): String {
    val db = requireDatabase()
    db.beginTransaction()
    try {
      val result = PublicRepositories(db).execute(operation, payload)
      db.setTransactionSuccessful()
      return result?.toString() ?: "null"
    } finally { db.endTransaction() }
  }

  fun lock(): String {
    closeHandles(); errorCode = null
    return status()
  }

  fun removeLocalData(): String = encrypted.withLifecycleLock { lock(); encrypted.removeLocalData(); errorCode = null; status() }
  private fun closeHandles() {
    val active = engine; engine = null; database = null
    try { active?.closeEngine() } finally { try { active?.destroy() } finally { encrypted.close() } }
  }
  private fun poison(code: String) {
    errorCode = code
    try { engine?.poison() } catch (_: Exception) { }
    try { closeHandles() } catch (_: Exception) { }
  }
  private fun base64(bytes: ByteArray) = Base64.encodeToString(bytes, Base64.NO_WRAP)
}
