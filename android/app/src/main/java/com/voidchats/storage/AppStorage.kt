package com.voidchats.storage

import android.content.Context
import android.util.Base64
import com.facebook.react.bridge.Promise
import com.voidchats.BuildConfig
import java.util.UUID
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import net.zetetic.database.sqlcipher.SQLiteDatabase
import org.json.JSONObject
import org.json.JSONArray
import org.json.JSONTokener

/** One serialized SQLCipher boundary for account keys, Signal state and history. */
internal class AppStorage(private val context: Context, namespace: String = "void-local-v1") {
  private val encrypted = EncryptedDatabase(context, namespace)
  private var database: SQLiteDatabase? = null
  private var errorCode: String? = null
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
        catch (error: SignalFailure) { promise.reject(error.code, "The protected Signal operation could not be completed.") }
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
      transaction {
        AccountKeyStore(db).preserveExisting(identity)
        SignalStore(db).ensureIdentity()
      }
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
    initializeLocalIdentity()
  }

  /** Called only by explicit development setup or account registration. */
  private fun initializeLocalIdentity(): String = encrypted.withLifecycleLock {
    if (encrypted.exists()) return@withLifecycleLock open()
    try {
      val db = encrypted.open(create = true)
      database = db
      StorageMigrations(context).migrate(db)
      transaction {
        val localId = UUID.randomUUID().toString()
        val publicKey = AccountKeyStore(db).generate()
        db.execSQL("INSERT INTO local_identity(slot,local_device_id,client_handle,credential_identity_base64,signature_public_key_base64,account_void_id,created_at) VALUES(1,?,?,?,?,NULL,?)",
          arrayOf(localId, "account-$localId", base64(localId.toByteArray(Charsets.UTF_8)), base64(publicKey), System.currentTimeMillis()))
        SignalStore(db).ensureIdentity()
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
        val account = registration()
        put("accountUsername", if (it.isNull(4) || account == null || account.isNull("username")) JSONObject.NULL else account.getString("username"))
        put("accountApiUrl", if (it.isNull(4) || account == null) JSONObject.NULL else account.getString("api_url"))
      }
    }
  }

  private fun registration(): JSONObject? = requireDatabase().rawQuery("SELECT username,api_url,proof_signed,created_at FROM account_registration WHERE slot=1", emptyArray()).use {
    if (!it.moveToFirst()) null else JSONObject().put("username", if (it.isNull(0)) JSONObject.NULL else it.getString(0)).put("api_url", it.getString(1))
      .put("proof_signed", it.getInt(2)).put("created_at", if (it.isNull(3)) JSONObject.NULL else it.getString(3))
  }

  /** Reuses durable identity material, including after failures and restarts. */
  fun prepareAccountRegistration(apiUrl: String): String {
    RegistrationProof.validateApiUrl(apiUrl)
    initializeLocalIdentity()
    val identity = localIdentity() ?: throw StorageFailure("DATABASE_ERROR")
    val db = requireDatabase()
    db.beginTransaction()
    try {
      val current = registration()
      if (!identity.isNull("accountVoidId")) {
        if (current == null || current.getString("api_url") != apiUrl) throw StorageFailure("CONFLICT")
      } else if (current == null || !current.isNull("username") || current.getString("api_url") != apiUrl) {
        db.execSQL("INSERT INTO account_registration(slot,username,api_url,proof_signed,created_at) VALUES(1,NULL,?,0,NULL) ON CONFLICT(slot) DO UPDATE SET username=NULL,api_url=excluded.api_url,proof_signed=0,created_at=NULL", arrayOf(apiUrl))
      }
      db.setTransactionSuccessful()
    } finally { db.endTransaction() }
    return localIdentity()!!.toString()
  }

  fun signAccountChallenge(messageBase64: String): String {
    val identity = localIdentity() ?: throw StorageFailure("NOT_INITIALIZED")
    if (!identity.isNull("accountVoidId")) throw StorageFailure("CONFLICT")
    val pending = registration() ?: throw StorageFailure("INVALID_INPUT")
    require(pending.isNull("username"))
    val message = RegistrationProof.validate(messageBase64, identity.getString("signaturePublicKeyBase64"))
    // Keep the deployed registration framing and the original Ed25519 account key.
    val signature = AccountKeyStore(requireDatabase()).sign(message)
    val db = requireDatabase()
    db.beginTransaction()
    try {
      db.execSQL("UPDATE account_registration SET proof_signed=1 WHERE slot=1")
      db.setTransactionSuccessful()
    } finally { db.endTransaction() }
    return base64(signature)
  }

  fun completeAccountRegistration(accountJSON: String): String {
    require(accountJSON.length in 1..8192)
    val account = try {
      val parser = JSONTokener(accountJSON)
      val parsed = parser.nextValue() as? JSONObject ?: throw IllegalArgumentException()
      require(parser.nextClean() == '\u0000')
      parsed
    } catch (_: org.json.JSONException) { throw IllegalArgumentException() }
    val fields = setOf("void_id", "created_at", "api_url", "auth_public_key")
    require(account.length() == fields.size && account.keys().asSequence().all { it in fields && account.get(it) is String })
    val voidId = account.getString("void_id")
    require(Regex("^[1-9][0-9]{0,18}$").matches(voidId) && voidId.toLongOrNull() != null)
    require(RegistrationProof.timestampMillis(account.getString("created_at")) >= 0)
    val identity = localIdentity() ?: throw StorageFailure("NOT_INITIALIZED")
    val db = requireDatabase()
    db.beginTransaction()
    try {
      val pending = registration() ?: throw StorageFailure("INVALID_INPUT")
      if (pending.getString("api_url") != account.getString("api_url") ||
          pending.getInt("proof_signed") != 1 || identity.getString("signaturePublicKeyBase64") != account.getString("auth_public_key")) throw StorageFailure("CONFLICT")
      if (!identity.isNull("accountVoidId")) {
        if (identity.getString("accountVoidId") != voidId || pending.optString("created_at") != account.getString("created_at")) throw StorageFailure("CONFLICT")
      } else {
        db.execSQL("UPDATE local_identity SET account_void_id=? WHERE slot=1 AND account_void_id IS NULL", arrayOf(voidId))
        db.execSQL("UPDATE account_registration SET created_at=? WHERE slot=1", arrayOf(account.getString("created_at")))
      }
      db.setTransactionSuccessful()
    } finally { db.endTransaction() }
    return status()
  }

  fun requireDatabase(): SQLiteDatabase = database ?: throw StorageFailure(if (encrypted.exists()) "LOCKED" else "NOT_INITIALIZED")
  fun <T> transaction(operation: () -> T): T {
    val db = requireDatabase()
    db.beginTransaction()
    try { val result = operation(); db.setTransactionSuccessful(); return result }
    finally { db.endTransaction() }
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
    database = null
    encrypted.close()
  }
  private fun poison(code: String) {
    errorCode = code
    try { closeHandles() } catch (_: Exception) { }
  }
  private fun base64(bytes: ByteArray) = Base64.encodeToString(bytes, Base64.NO_WRAP)
}
