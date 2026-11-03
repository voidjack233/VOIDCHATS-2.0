package com.voidchats.storage

import android.util.Base64
import java.security.MessageDigest
import java.security.SecureRandom
import net.zetetic.database.sqlcipher.SQLiteDatabase
import org.json.JSONArray
import org.json.JSONObject
import org.signal.libsignal.protocol.IdentityKey
import org.signal.libsignal.protocol.IdentityKeyPair
import org.signal.libsignal.protocol.InvalidKeyIdException
import org.signal.libsignal.protocol.NoSessionException
import org.signal.libsignal.protocol.ReusedBaseKeyException
import org.signal.libsignal.protocol.SignalProtocolAddress
import org.signal.libsignal.protocol.ecc.ECKeyPair
import org.signal.libsignal.protocol.ecc.ECPublicKey
import org.signal.libsignal.protocol.kem.KEMKeyPair
import org.signal.libsignal.protocol.kem.KEMKeyType
import org.signal.libsignal.protocol.state.IdentityKeyStore
import org.signal.libsignal.protocol.state.KyberPreKeyRecord
import org.signal.libsignal.protocol.state.KyberPreKeyStore
import org.signal.libsignal.protocol.state.PreKeyRecord
import org.signal.libsignal.protocol.state.PreKeyStore
import org.signal.libsignal.protocol.state.SessionRecord
import org.signal.libsignal.protocol.state.SessionStore
import org.signal.libsignal.protocol.state.SignedPreKeyRecord
import org.signal.libsignal.protocol.state.SignedPreKeyStore

/** Serialized by AppStorage. Every callback loads a fresh record from SQLCipher. */
internal class SignalStore(private val db: SQLiteDatabase) :
  IdentityKeyStore, SessionStore, PreKeyStore, SignedPreKeyStore, KyberPreKeyStore {
  private fun writing() { check(db.inTransaction()) { "A protected Signal transaction is required." } }
  private fun base64(bytes: ByteArray) = Base64.encodeToString(bytes, Base64.NO_WRAP)
  private fun validateAddress(address: SignalProtocolAddress) {
    require(address.deviceId == 1)
    require(address.name.toLongOrNull()?.let { it > 0 && it.toString() == address.name } == true)
  }
  private fun blob(sql: String, vararg args: String): ByteArray? = db.rawQuery(sql, args).use {
    if (it.moveToFirst()) it.getBlob(0) else null
  }
  private fun <T> decoded(bytes: ByteArray, decode: (ByteArray) -> T): T = try { decode(bytes) } finally { bytes.fill(0) }
  private fun record(kind: String, id: Int): ByteArray =
    blob("SELECT record FROM signal_prekeys WHERE kind=? AND id=?", kind, id.toString())
      ?: throw InvalidKeyIdException("The requested Signal prekey is unavailable.")
  private fun contains(kind: String, id: Int): Boolean = db.rawQuery(
    "SELECT 1 FROM signal_prekeys WHERE kind=? AND id=?", arrayOf(kind, id.toString())
  ).use { it.moveToFirst() }
  private fun store(kind: String, id: Int, bytes: ByteArray) {
    writing()
    require(id in 1..16_777_214)
    try {
      db.execSQL("INSERT INTO signal_prekeys(kind,id,record) VALUES(?,?,?) ON CONFLICT(kind,id) DO UPDATE SET record=excluded.record", arrayOf(kind, id, bytes))
    } finally { bytes.fill(0) }
  }
  private fun remove(kind: String, id: Int) {
    writing()
    db.execSQL("DELETE FROM signal_prekeys WHERE kind=? AND id=?", arrayOf(kind, id))
  }
  private fun ids(kind: String, unpublishedOnly: Boolean = false): List<Int> = db.rawQuery(
    "SELECT id FROM signal_prekeys WHERE kind=?" + (if (unpublishedOnly) " AND published=0 AND last_resort=0" else "") + " ORDER BY id",
    arrayOf(kind)
  ).use { cursor -> buildList { while (cursor.moveToNext()) add(cursor.getInt(0)) } }

  /** Creates only the separate Signal identity; never changes the account signer. */
  fun ensureIdentity() {
    writing()
    val exists = db.rawQuery("SELECT 1 FROM signal_local_identity WHERE slot=1", emptyArray()).use { it.moveToFirst() }
    if (exists) { getIdentityKeyPair(); getLocalRegistrationId(); return }
    val previous = db.rawQuery("SELECT 1 FROM signal_sessions UNION ALL SELECT 1 FROM signal_prekeys UNION ALL SELECT 1 FROM signal_identities UNION ALL SELECT 1 FROM signal_kyber_uses UNION ALL SELECT 1 FROM signal_threads UNION ALL SELECT 1 FROM signal_publication UNION ALL SELECT 1 FROM signal_outbox UNION ALL SELECT 1 FROM signal_inbox UNION ALL SELECT 1 FROM signal_drafts LIMIT 1", emptyArray()).use { it.moveToFirst() }
    if (previous) throw StorageFailure("SIGNAL_IDENTITY_MISSING")
    val pair = IdentityKeyPair.generate().serialize()
    try {
      val registration = SecureRandom().nextInt(16_380) + 1
      db.execSQL("INSERT INTO signal_local_identity(slot,key_pair,registration_id,next_prekey_id) VALUES(1,?,?,1)", arrayOf(pair, registration))
    } finally { pair.fill(0) }
  }

  override fun getIdentityKeyPair(): IdentityKeyPair = decoded(
    blob("SELECT key_pair FROM signal_local_identity WHERE slot=1") ?: throw StorageFailure("SIGNAL_IDENTITY_MISSING"),
    ::IdentityKeyPair
  )

  override fun getLocalRegistrationId(): Int = db.rawQuery(
    "SELECT registration_id FROM signal_local_identity WHERE slot=1", emptyArray()
  ).use {
    if (!it.moveToFirst()) throw StorageFailure("SIGNAL_IDENTITY_MISSING")
    it.getInt(0).also { value -> if (value !in 1..16_380) throw StorageFailure("DATABASE_ERROR") }
  }

  /** Call only after verifying this Signal identity's account-key signature. */
  fun bindIdentity(peer: String, key: IdentityKey, deviceId: Int = 1) {
    writing()
    val address = SignalProtocolAddress(peer, deviceId); validateAddress(address)
    val incoming = key.serialize()
    val existing = blob("SELECT identity_key FROM signal_identities WHERE peer_void_id=? AND device_id=?", peer, deviceId.toString())
    try {
      if (existing != null) {
        if (!MessageDigest.isEqual(existing, incoming)) throw StorageFailure("PEER_KEY_CHANGED")
      } else {
        db.execSQL("INSERT INTO signal_identities(peer_void_id,device_id,identity_key) VALUES(?,?,?)", arrayOf(peer, deviceId, incoming))
      }
    } finally { incoming.fill(0); existing?.fill(0) }
  }

  override fun saveIdentity(address: SignalProtocolAddress, identityKey: IdentityKey): IdentityKeyStore.IdentityChange {
    writing(); validateAddress(address)
    // Libsignal cannot introduce or replace a pin by itself. The account binding
    // must already have been verified and pinned by the native coordinator.
    if (!isTrustedIdentity(address, identityKey, IdentityKeyStore.Direction.RECEIVING)) throw StorageFailure("PEER_KEY_CHANGED")
    return IdentityKeyStore.IdentityChange.NEW_OR_UNCHANGED
  }

  override fun isTrustedIdentity(address: SignalProtocolAddress, identityKey: IdentityKey, direction: IdentityKeyStore.Direction): Boolean {
    validateAddress(address)
    val saved = blob("SELECT identity_key FROM signal_identities WHERE peer_void_id=? AND device_id=?", address.name, address.deviceId.toString()) ?: return false
    val incoming = identityKey.serialize()
    return try { MessageDigest.isEqual(saved, incoming) } finally { saved.fill(0); incoming.fill(0) }
  }

  override fun getIdentity(address: SignalProtocolAddress): IdentityKey? {
    validateAddress(address)
    return blob("SELECT identity_key FROM signal_identities WHERE peer_void_id=? AND device_id=?", address.name, address.deviceId.toString())?.let { decoded(it, ::IdentityKey) }
  }

  override fun loadSession(address: SignalProtocolAddress): SessionRecord {
    validateAddress(address)
    return blob("SELECT record FROM signal_sessions WHERE peer_void_id=? AND device_id=?", address.name, address.deviceId.toString())?.let { decoded(it, ::SessionRecord) } ?: SessionRecord()
  }

  override fun loadExistingSessions(addresses: MutableList<SignalProtocolAddress>): MutableList<SessionRecord> = addresses.map { address ->
    if (!containsSession(address)) throw NoSessionException("The requested Signal session is unavailable.")
    loadSession(address)
  }.toMutableList()

  override fun getSubDeviceSessions(name: String): MutableList<Int> = db.rawQuery(
    "SELECT device_id FROM signal_sessions WHERE peer_void_id=? ORDER BY device_id", arrayOf(name)
  ).use { cursor -> buildList { while (cursor.moveToNext()) add(cursor.getInt(0)) }.toMutableList() }

  override fun storeSession(address: SignalProtocolAddress, record: SessionRecord) {
    writing(); validateAddress(address)
    val bytes = record.serialize()
    try {
      db.execSQL("INSERT INTO signal_sessions(peer_void_id,device_id,record) VALUES(?,?,?) ON CONFLICT(peer_void_id,device_id) DO UPDATE SET record=excluded.record", arrayOf(address.name, address.deviceId, bytes))
    } finally { bytes.fill(0) }
  }

  override fun containsSession(address: SignalProtocolAddress): Boolean {
    validateAddress(address)
    return blob("SELECT record FROM signal_sessions WHERE peer_void_id=? AND device_id=?", address.name, address.deviceId.toString())?.let { decoded(it, ::SessionRecord).hasSenderChain() } ?: false
  }

  override fun deleteSession(address: SignalProtocolAddress) {
    writing(); validateAddress(address)
    db.execSQL("DELETE FROM signal_sessions WHERE peer_void_id=? AND device_id=?", arrayOf(address.name, address.deviceId))
  }

  override fun deleteAllSessions(name: String) {
    writing()
    db.execSQL("DELETE FROM signal_sessions WHERE peer_void_id=?", arrayOf(name))
  }

  override fun loadPreKey(preKeyId: Int): PreKeyRecord = decoded(record("ec", preKeyId), ::PreKeyRecord)
  override fun storePreKey(preKeyId: Int, record: PreKeyRecord) { store("ec", preKeyId, record.serialize()) }
  override fun containsPreKey(preKeyId: Int) = contains("ec", preKeyId)
  override fun removePreKey(preKeyId: Int) { remove("ec", preKeyId) }

  override fun loadSignedPreKey(signedPreKeyId: Int): SignedPreKeyRecord = decoded(record("signed", signedPreKeyId), ::SignedPreKeyRecord)
  override fun loadSignedPreKeys(): MutableList<SignedPreKeyRecord> = ids("signed").map(::loadSignedPreKey).toMutableList()
  override fun storeSignedPreKey(signedPreKeyId: Int, record: SignedPreKeyRecord) { store("signed", signedPreKeyId, record.serialize()) }
  override fun containsSignedPreKey(signedPreKeyId: Int) = contains("signed", signedPreKeyId)
  override fun removeSignedPreKey(signedPreKeyId: Int) {
    remove("signed", signedPreKeyId)
    db.execSQL("DELETE FROM signal_kyber_uses WHERE signed_prekey_id=?", arrayOf(signedPreKeyId))
  }

  override fun loadKyberPreKey(kyberPreKeyId: Int): KyberPreKeyRecord = decoded(record("kyber", kyberPreKeyId), ::KyberPreKeyRecord)
  override fun loadKyberPreKeys(): MutableList<KyberPreKeyRecord> = ids("kyber").map(::loadKyberPreKey).toMutableList()
  override fun storeKyberPreKey(kyberPreKeyId: Int, record: KyberPreKeyRecord) { store("kyber", kyberPreKeyId, record.serialize()) }
  override fun containsKyberPreKey(kyberPreKeyId: Int) = contains("kyber", kyberPreKeyId)

  override fun markKyberPreKeyUsed(kyberPreKeyId: Int, signedPreKeyId: Int, baseKey: ECPublicKey) {
    writing()
    val lastResort = db.rawQuery("SELECT last_resort FROM signal_prekeys WHERE kind='kyber' AND id=?", arrayOf(kyberPreKeyId.toString())).use {
      if (!it.moveToFirst()) throw InvalidKeyIdException("The requested Signal prekey is unavailable.")
      it.getInt(0) == 1
    }
    if (!lastResort) { remove("kyber", kyberPreKeyId); return }
    val encoded = baseKey.serialize()
    try {
      db.compileStatement("INSERT OR IGNORE INTO signal_kyber_uses(kyber_prekey_id,signed_prekey_id,base_key) VALUES(?,?,?)").use { statement ->
        statement.bindLong(1, kyberPreKeyId.toLong()); statement.bindLong(2, signedPreKeyId.toLong()); statement.bindBlob(3, encoded)
        if (statement.executeInsert() == -1L) throw ReusedBaseKeyException()
      }
    } finally { encoded.fill(0) }
  }

  private fun nextId(): Int {
    writing()
    val id = db.rawQuery("SELECT next_prekey_id FROM signal_local_identity WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst()) throw StorageFailure("SIGNAL_IDENTITY_MISSING")
      it.getInt(0)
    }
    if (id !in 1..16_777_214) throw StorageFailure("SIGNAL_PREKEY_LIMIT")
    db.execSQL("UPDATE signal_local_identity SET next_prekey_id=? WHERE slot=1", arrayOf(id + 1))
    return id
  }

  private fun generateInitialPrekeys() {
    writing(); ensureIdentity()
    val initialized = db.rawQuery("SELECT initial_prekeys_generated FROM signal_local_identity WHERE slot=1", emptyArray()).use { it.moveToFirst(); it.getInt(0) == 1 }
    if (initialized) return
    val signer = getIdentityKeyPair().privateKey
    val timestamp = System.currentTimeMillis()
    val signedId = nextId(); val signed = ECKeyPair.generate()
    storeSignedPreKey(signedId, SignedPreKeyRecord(signedId, timestamp, signed, signer.calculateSignature(signed.publicKey.serialize())))
    val lastResortId = nextId(); val lastResort = KEMKeyPair.generate(KEMKeyType.KYBER_1024)
    storeKyberPreKey(lastResortId, KyberPreKeyRecord(lastResortId, timestamp, lastResort, signer.calculateSignature(lastResort.publicKey.serialize())))
    db.execSQL("UPDATE signal_prekeys SET last_resort=1 WHERE kind='kyber' AND id=?", arrayOf(lastResortId))
    repeat(20) {
      val ecId = nextId(); storePreKey(ecId, PreKeyRecord(ecId, ECKeyPair.generate()))
      val kyberId = nextId(); val key = KEMKeyPair.generate(KEMKeyType.KYBER_1024)
      storeKyberPreKey(kyberId, KyberPreKeyRecord(kyberId, timestamp, key, signer.calculateSignature(key.publicKey.serialize())))
    }
    db.execSQL("UPDATE signal_local_identity SET initial_prekeys_generated=1,signed_prekey_id=?,kyber_last_resort_id=? WHERE slot=1", arrayOf(signedId, lastResortId))
  }

  private fun signedPublic(record: SignedPreKeyRecord) = JSONObject().put("key_id", record.id)
    .put("public_key", base64(record.keyPair.publicKey.serialize())).put("signature", base64(record.signature))
  private fun kyberPublic(record: KyberPreKeyRecord) = JSONObject().put("key_id", record.id)
    .put("public_key", base64(record.keyPair.publicKey.serialize())).put("signature", base64(record.signature))

  /** Public material only. Commit its private records before publishing this JSON. */
  fun prekeyPublication(): JSONObject {
    writing(); generateInitialPrekeys()
    val current = db.rawQuery("SELECT signed_prekey_id,kyber_last_resort_id FROM signal_local_identity WHERE slot=1", emptyArray()).use {
      if (!it.moveToFirst() || it.isNull(0) || it.isNull(1)) throw StorageFailure("DATABASE_ERROR")
      Pair(it.getInt(0), it.getInt(1))
    }
    return JSONObject().put("device_id", 1).put("registration_id", getLocalRegistrationId())
      .put("identity_key", base64(getIdentityKeyPair().publicKey.serialize()))
      .put("signed_prekey", signedPublic(loadSignedPreKey(current.first)))
      .put("kyber_last_resort", kyberPublic(loadKyberPreKey(current.second)))
      .put("one_time_prekeys", JSONArray().apply { for (id in ids("ec", true)) { val key = loadPreKey(id); put(JSONObject().put("key_id", id).put("public_key", base64(key.keyPair.publicKey.serialize()))) } })
      .put("one_time_kyber_prekeys", JSONArray().apply { for (id in ids("kyber", true)) put(kyberPublic(loadKyberPreKey(id))) })
  }

  /** Call only after the server confirms this exact durable public publication. */
  fun markPublished(publication: JSONObject) {
    writing()
    for ((field, kind) in listOf("one_time_prekeys" to "ec", "one_time_kyber_prekeys" to "kyber")) {
      val keys = publication.getJSONArray(field)
      for (index in 0 until keys.length()) db.execSQL("UPDATE signal_prekeys SET published=1 WHERE kind=? AND id=?", arrayOf(kind, keys.getJSONObject(index).getInt("key_id")))
    }
  }
}
