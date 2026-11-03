package com.voidchats.storage

import java.security.SecureRandom
import net.zetetic.database.sqlcipher.SQLiteDatabase
import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import org.json.JSONArray
import org.json.JSONObject

/** Account Ed25519 authentication is independent of the Signal identity.
 * Legacy import reads the checkpoint format; it never loads or modifies MLS. */
internal class AccountKeyStore(private val db: SQLiteDatabase) {
  fun generate(): ByteArray {
    check(db.inTransaction())
    val seed = ByteArray(32).also { SecureRandom().nextBytes(it) }
    try {
      val publicKey = Ed25519PrivateKeyParameters(seed, 0).generatePublicKey().encoded
      db.execSQL("INSERT INTO account_signing_key(slot,seed,public_key) VALUES(1,?,?)", arrayOf(seed, publicKey))
      return publicKey
    } finally { seed.fill(0) }
  }

  fun preserveExisting(identity: JSONObject) {
    check(db.inTransaction())
    val expected = MessagingProtocol.bytes(identity.getString("signaturePublicKeyBase64"), 32)
    val present = db.rawQuery("SELECT public_key FROM account_signing_key WHERE slot=1", emptyArray()).use {
      if (it.moveToFirst()) it.getBlob(0) else null
    }
    if (present != null) {
      if (!present.contentEquals(expected)) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      // Prove that the stored private key still corresponds to the registered key.
      val probe = "VOID:local-account-key-check:v1".toByteArray()
      if (!verify(expected, probe, sign(probe))) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      return
    }
    val metadata = db.rawQuery("SELECT revision,length(snapshot) FROM native_mls_state WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst()) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      Pair(it.getLong(0), it.getInt(1))
    }
    if (metadata.first < 1 || metadata.second !in 1..64 * 1024 * 1024) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
    val bytes = ByteArray(metadata.second)
    try {
      var offset = 0
      while (offset < bytes.size) {
        val length = minOf(256 * 1024, bytes.size - offset)
        db.rawQuery("SELECT substr(snapshot,?,?) FROM native_mls_state WHERE slot=1 AND revision=?",
          arrayOf((offset + 1).toString(), length.toString(), metadata.first.toString())).use {
          if (!it.moveToFirst()) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
          val chunk = it.getBlob(0)
          try { if (chunk.size != length) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE"); chunk.copyInto(bytes, offset) }
          finally { chunk.fill(0) }
        }
        offset += length
      }
      val snapshot = JSONObject(bytes.toString(Charsets.UTF_8))
      if (snapshot.getInt("format") != 1 || snapshot.getLong("revision") != metadata.first ||
          snapshot.getString("openmls_version") != "0.9.1" ||
          snapshot.getString("provider_version") != "openmls_rust_crypto=0.6.0;openmls_memory_storage=0.6.0") throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      val clients = snapshot.getJSONArray("clients")
      var selected: JSONObject? = null
      for (index in 0 until clients.length()) {
        val entry = clients.getJSONArray(index)
        if (entry.getString(0) == identity.getString("clientHandle")) {
          if (selected != null) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
          selected = entry.getJSONObject(1)
        }
      }
      val client = selected ?: throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      if (!byteArray(client.getJSONArray("signature_public_key")).contentEquals(expected)) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      val records = client.getJSONArray("storage")
      var imported = false
      for (index in 0 until records.length()) {
        val entry = records.getJSONArray(index)
        val label = byteArray(entry.getJSONArray(0))
        if (!label.take(16).toByteArray().contentEquals("SignatureKeyPair".toByteArray())) continue
        val encoded = byteArray(entry.getJSONArray(1))
        try {
          val key = JSONObject(encoded.toString(Charsets.UTF_8))
          if (key.optString("signature_scheme") != "ED25519" ||
              !byteArray(key.getJSONArray("public")).contentEquals(expected)) continue
          if (imported) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
          val seed = byteArray(key.getJSONArray("private"))
          try {
            if (seed.size != 32 || !Ed25519PrivateKeyParameters(seed, 0).generatePublicKey().encoded.contentEquals(expected)) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
            db.execSQL("INSERT INTO account_signing_key(slot,seed,public_key) VALUES(1,?,?)", arrayOf(seed, expected))
            imported = true
          } finally { seed.fill(0) }
        } finally { encoded.fill(0) }
      }
      if (!imported) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
    } catch (error: StorageFailure) { throw error }
    catch (_: Exception) { throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE") }
    finally { bytes.fill(0) }
  }

  fun sign(message: ByteArray): ByteArray {
    val seed = db.rawQuery("SELECT seed FROM account_signing_key WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst()) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      it.getBlob(0)
    }
    try {
      if (seed.size != 32) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
      return Ed25519Signer().run {
        init(true, Ed25519PrivateKeyParameters(seed, 0))
        update(message, 0, message.size)
        generateSignature()
      }
    } finally { seed.fill(0) }
  }

  private fun byteArray(value: JSONArray): ByteArray = ByteArray(value.length()) { index ->
    val number = value.getInt(index)
    if (number !in 0..255) throw StorageFailure("ACCOUNT_KEY_UNAVAILABLE")
    number.toByte()
  }

  companion object {
    fun verify(publicKey: ByteArray, message: ByteArray, signature: ByteArray): Boolean {
      if (publicKey.size != 32 || signature.size != 64) return false
      return Ed25519Signer().run {
        init(false, Ed25519PublicKeyParameters(publicKey, 0))
        update(message, 0, message.size)
        verifySignature(signature)
      }
    }
  }
}
