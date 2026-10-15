package com.voidchats.storage

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.nio.ByteBuffer
import java.security.KeyStore
import java.security.SecureRandom
import java.util.concurrent.ConcurrentHashMap
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import net.zetetic.database.DatabaseErrorHandler
import net.zetetic.database.Logger
import net.zetetic.database.NoopTarget
import net.zetetic.database.sqlcipher.SQLiteDatabase

internal class StorageFailure(val code: String = "STORAGE_UNAVAILABLE") : RuntimeException("Protected local storage is unavailable.")

/** Only native code can obtain the key. Neither an absent key nor corruption resets data. */
internal class EncryptedDatabase(private val context: Context, namespace: String = "void-local-v1") {
  // The manifest uses one app process. All React contexts share each namespace lock.
  private companion object {
    val lifecycleLocks = ConcurrentHashMap<String, Any>()
    val openedOwners = ConcurrentHashMap<String, MutableSet<EncryptedDatabase>>()
  }
  init { require(namespace.matches(Regex("[A-Za-z0-9_-]{1,80}"))) }
  val directory = File(context.noBackupFilesDir, namespace)
  val databaseFile = File(directory, "store.db")
  val wrappedKeyFile = File(directory, "database-key.v1")
  val keyAlias = "${context.packageName}.$namespace.database-key"
  private val canonicalDirectory = directory.canonicalPath
  private val lifecycleLock = lifecycleLocks.getOrPut(canonicalDirectory) { Any() }
  private val owners = openedOwners.getOrPut(canonicalDirectory) { mutableSetOf() }
  private val aad = keyAlias.toByteArray(Charsets.UTF_8)
  private var databaseKey: ByteArray? = null
  private var active: SQLiteDatabase? = null
  private fun keyStore() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

  fun exists(): Boolean = databaseFile.exists() || wrappedKeyFile.exists() || keyStore().containsAlias(keyAlias)

  fun <T> withLifecycleLock(operation: () -> T): T = synchronized(lifecycleLock, operation)

  fun open(create: Boolean = false): SQLiteDatabase = withLifecycleLock {
    active?.let { return@withLifecycleLock it }
    try {
      System.loadLibrary("sqlcipher")
      Logger.setTarget(NoopTarget())
      val existing = exists()
      if (existing && (!databaseFile.isFile || !wrappedKeyFile.isFile || !keyStore().containsAlias(keyAlias))) {
        throw StorageFailure("STORAGE_KEY_UNAVAILABLE")
      }
      if (existing && databaseFile.length() < 4096L) throw StorageFailure("STORAGE_CORRUPT")
      if (!existing && !create) throw StorageFailure("STORAGE_NOT_INITIALIZED")
      val passphrase = if (existing) unwrapKey() else createWrappedKey()
      databaseKey = passphrase
      val flags = SQLiteDatabase.OPEN_READWRITE or SQLiteDatabase.NO_LOCALIZED_COLLATORS or
        (if (existing) 0 else SQLiteDatabase.CREATE_IF_NECESSARY)
      // SQLCipher's default corruption handler deletes databases. Never use it here.
      val db = SQLiteDatabase.openDatabase(databaseFile.absolutePath, passphrase, null, flags,
        DatabaseErrorHandler { _, _ -> throw StorageFailure("STORAGE_CORRUPT") }, null)
      active = db
      db.rawQuery("SELECT count(*) FROM sqlite_master", emptyArray()).use { if (!it.moveToFirst()) throw StorageFailure() }
      if (!scalar(db, "PRAGMA cipher_version").startsWith("4.") || scalar(db, "PRAGMA cipher_status") != "1") throw StorageFailure("STORAGE_ENCRYPTION_UNAVAILABLE")
      // SQLCipher PRAGMAs can return scalar rows even when assigning a setting.
      for (pragma in listOf("cipher_log_level=NONE", "cipher_memory_security=ON", "foreign_keys=ON", "secure_delete=ON", "temp_store=MEMORY", "synchronous=FULL")) {
        db.rawQuery("PRAGMA $pragma", emptyArray()).use { while (it.moveToNext()) { } }
      }
      if (scalar(db, "PRAGMA journal_mode=DELETE").lowercase() != "delete" ||
          scalar(db, "PRAGMA synchronous") != "2" || scalar(db, "PRAGMA foreign_keys") != "1" ||
          scalar(db, "PRAGMA secure_delete") != "1" || scalar(db, "PRAGMA temp_store") != "2" ||
          scalar(db, "PRAGMA cipher_memory_security") != "1") throw StorageFailure("STORAGE_ENCRYPTION_UNAVAILABLE")
      owners.add(this)
      return@withLifecycleLock db
    } catch (error: Throwable) {
      close()
      if (error is StorageFailure) throw error
      throw StorageFailure("STORAGE_KEY_UNAVAILABLE")
    }
  }

  private fun scalar(db: SQLiteDatabase, sql: String): String = db.rawQuery(sql, emptyArray()).use {
    if (!it.moveToFirst() || it.isNull(0)) throw StorageFailure("STORAGE_ENCRYPTION_UNAVAILABLE")
    it.getString(0)
  }

  private fun createWrappedKey(): ByteArray {
    if (!directory.mkdirs() && !directory.isDirectory) throw StorageFailure()
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    generator.init(KeyGenParameterSpec.Builder(keyAlias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
      .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setRandomizedEncryptionRequired(true).build())
    val wrappingKey = generator.generateKey()
    val key = ByteArray(32).also { SecureRandom().nextBytes(it) }
    try {
      val cipher = Cipher.getInstance("AES/GCM/NoPadding")
      cipher.init(Cipher.ENCRYPT_MODE, wrappingKey)
      cipher.updateAAD(aad)
      val wrapped = cipher.doFinal(key)
      val encoded = ByteBuffer.allocate(8 + cipher.iv.size + wrapped.size).putInt(1).putInt(cipher.iv.size).put(cipher.iv).put(wrapped).array()
      val file = AtomicFile(wrappedKeyFile)
      val output = file.startWrite()
      try { output.write(encoded); file.finishWrite(output) } catch (error: Throwable) { file.failWrite(output); throw error }
      return key
    } catch (error: Throwable) { key.fill(0); throw error }
  }

  private fun unwrapKey(): ByteArray {
    if (wrappedKeyFile.length() != 68L) throw StorageFailure("STORAGE_KEY_UNAVAILABLE")
    val encoded = ByteBuffer.wrap(wrappedKeyFile.readBytes())
    if (encoded.int != 1 || encoded.int != 12) throw StorageFailure("STORAGE_KEY_UNAVAILABLE")
    val iv = ByteArray(12).also { encoded.get(it) }
    val ciphertext = ByteArray(48).also { encoded.get(it) }
    val wrappingKey = keyStore().getKey(keyAlias, null) as? SecretKey ?: throw StorageFailure("STORAGE_KEY_UNAVAILABLE")
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.DECRYPT_MODE, wrappingKey, GCMParameterSpec(128, iv))
    cipher.updateAAD(aad)
    return cipher.doFinal(ciphertext).also { if (it.size != 32) { it.fill(0); throw StorageFailure("STORAGE_KEY_UNAVAILABLE") } }
  }

  fun close() {
    withLifecycleLock {
      try { active?.close() } finally { active = null; databaseKey?.fill(0); databaseKey = null; owners.remove(this) }
    }
  }

  /** Called only by the explicit local-data deletion action, never during recovery. */
  fun removeLocalData() {
    withLifecycleLock {
    if (owners.any { it !== this }) throw StorageFailure("OPERATION_FAILED")
    close()
    for (name in listOf("store.db", "store.db-journal", "store.db-wal", "store.db-shm", "database-key.v1", "database-key.v1.bak", "database-key.v1.new")) {
      val file = File(directory, name)
      if (file.exists() && !file.delete()) throw StorageFailure()
    }
    keyStore().deleteEntry(keyAlias)
    if (directory.isDirectory && directory.list()?.isEmpty() == true) directory.delete()
    }
  }
}
