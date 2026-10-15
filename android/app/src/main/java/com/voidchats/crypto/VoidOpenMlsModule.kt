package com.voidchats.crypto

import android.util.Base64
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.bridge.WritableMap
import com.voidchats.BuildConfig
import com.voidchats.openmls.ClientInfo
import com.voidchats.openmls.GroupInfo
import com.voidchats.openmls.MlsEngine
import com.voidchats.storage.AppStorage

/** Public transport data only; private state is checkpointed inside the SQLCipher transaction. */
internal class VoidOpenMlsModule(context: ReactApplicationContext, private val storage: AppStorage) : ReactContextBaseJavaModule(context) {
  override fun getName() = "VoidOpenMls"
  private fun call(promise: Promise, vararg inputs: String, mutate: Boolean = true, operation: (MlsEngine) -> Any?) {
    if (inputs.any { it.length > 1_398_104 }) { promise.reject("MLS_INVALID_INPUT", "The MLS input exceeds the supported size."); return }
    storage.submit(promise) { if (mutate) storage.mutateMls(operation = operation) else operation(storage.requireEngine()) }
  }
  private fun bytes(encoded: String): ByteArray {
    require(encoded.length <= 1_398_104)
    return Base64.decode(encoded, Base64.NO_WRAP).also { require(base64(it) == encoded) }
  }
  private fun base64(value: ByteArray) = Base64.encodeToString(value, Base64.NO_WRAP)
  private fun client(value: ClientInfo): WritableMap = Arguments.createMap().apply {
    putString("clientHandle", value.clientHandle)
    putString("credentialIdentityBase64", base64(value.credentialIdentity))
    putString("signaturePublicKeyBase64", base64(value.signaturePublicKey))
  }
  private fun group(value: GroupInfo): WritableMap = Arguments.createMap().apply {
    putString("groupIdBase64", base64(value.groupId)); putString("epoch", value.epoch.toString()); putDouble("memberCount", value.memberCount.toDouble())
  }
  @ReactMethod fun initialize(promise: Promise) = call(promise, mutate = false) {
    val info = it.engineInfo()
    Arguments.createMap().apply {
      putString("openmlsVersion", info.openmlsVersion); putString("ciphersuite", info.ciphersuite)
      putString("protocolVersion", info.protocolVersion); putBoolean("durablePersistence", info.durablePersistence)
    }
  }
  @ReactMethod fun listClients(promise: Promise) = call(promise, mutate = false) {
    Arguments.createArray().apply { for (value in it.listClients()) pushMap(client(value)) }
  }
  @ReactMethod fun readStoredDecryptions(beforeJSON: String, limit: Double, promise: Promise) = storage.submit(promise) {
    try { storage.readStoredDecryptions(beforeJSON, limit) } catch (_: org.json.JSONException) { throw IllegalArgumentException() }
  }
  @ReactMethod fun createClient(identityBase64: String, promise: Promise) {
    promise.reject("MLS_INVALID_STATE", "Create the development local identity explicitly through protected local storage.")
  }
  @ReactMethod fun generateKeyPackage(clientHandle: String, promise: Promise) = call(promise, clientHandle) {
    storage.requirePrimary(clientHandle)
    val encoded = base64(it.generateKeyPackage(clientHandle))
    storage.requireDatabase().execSQL("INSERT INTO key_packages(id,client_handle,package_base64,created_at) VALUES(?,?,?,?)",
      arrayOf(java.util.UUID.randomUUID().toString(), clientHandle, encoded, System.currentTimeMillis()))
    encoded
  }
  @ReactMethod fun createGroup(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64)
    val result = it.createGroup(clientHandle, id); storage.journal("create_group", id, null, null); group(result)
  }
  @ReactMethod fun addMember(clientHandle: String, groupIdBase64: String, keyPackageBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, keyPackageBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64)
    val result = it.addMember(clientHandle, id, bytes(keyPackageBase64))
    storage.journal("add_member_commit", id, null, result.commit); storage.journal("add_member_welcome", id, null, result.welcome)
    Arguments.createMap().apply { putString("commitBase64", base64(result.commit)); putString("welcomeBase64", base64(result.welcome)) }
  }
  @ReactMethod fun mergePendingCommit(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64)
    val result = it.mergePendingCommit(clientHandle, id); storage.journal("merge_pending_commit", id, null, null); group(result)
  }
  @ReactMethod fun processWelcome(clientHandle: String, welcomeBase64: String, promise: Promise) = call(promise, clientHandle, welcomeBase64) {
    storage.requirePrimary(clientHandle); val welcome = bytes(welcomeBase64)
    val result = it.processWelcome(clientHandle, welcome); storage.journal("process_welcome", result.groupId, null, welcome); group(result)
  }
  @ReactMethod fun encrypt(clientHandle: String, groupIdBase64: String, plaintextBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, plaintextBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64); val plaintext = bytes(plaintextBase64)
    try {
      val ciphertext = it.encrypt(clientHandle, id, plaintext)
      try { storage.journal("encrypt", id, plaintext, ciphertext); base64(ciphertext) } finally { ciphertext.fill(0) }
    } finally { plaintext.fill(0) }
  }
  @ReactMethod fun decrypt(clientHandle: String, groupIdBase64: String, messageBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, messageBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64); val ciphertext = bytes(messageBase64)
    val result = it.decrypt(clientHandle, id, ciphertext)
    try {
      storage.journal("decrypt", id, result.plaintext, ciphertext, result.senderIdentity, result.senderSignaturePublicKey, result.epoch.toString())
      Arguments.createMap().apply {
        putString("plaintextBase64", base64(result.plaintext)); putString("senderIdentityBase64", base64(result.senderIdentity))
        putString("senderSignaturePublicKeyBase64", base64(result.senderSignaturePublicKey)); putString("epoch", result.epoch.toString())
      }
    } finally { result.plaintext.fill(0); ciphertext.fill(0) }
  }
  @ReactMethod fun selfUpdate(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64); val commit = it.selfUpdate(clientHandle, id)
    storage.journal("self_update", id, null, commit); base64(commit)
  }
  @ReactMethod fun processCommit(clientHandle: String, groupIdBase64: String, commitBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, commitBase64) {
    storage.requirePrimary(clientHandle); val id = bytes(groupIdBase64); val commit = bytes(commitBase64)
    val result = it.processCommit(clientHandle, id, commit); storage.journal("process_commit", id, null, commit); group(result)
  }
  @ReactMethod fun groupInfo(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, mutate = false) {
    storage.requirePrimary(clientHandle); group(it.groupInfo(clientHandle, bytes(groupIdBase64)))
  }
  private fun protectedLifecycle(promise: Promise) = promise.reject("MLS_INVALID_STATE", "The protected local identity remains active until explicit local-data removal.")
  @ReactMethod fun suspendClient(clientHandle: String, promise: Promise) = protectedLifecycle(promise)
  @ReactMethod fun restoreClient(snapshotHandle: String, promise: Promise) = protectedLifecycle(promise)
  @ReactMethod fun releaseClient(clientHandle: String, promise: Promise) = protectedLifecycle(promise)
  @ReactMethod fun discardSnapshot(snapshotHandle: String, promise: Promise) = protectedLifecycle(promise)
  @ReactMethod fun runSelfTest(promise: Promise) {
    if (!BuildConfig.DEBUG) { promise.reject("MLS_UNAVAILABLE", "The MLS self-test is available only in development builds."); return }
    storage.submit(promise) {
      val diagnostic = MlsEngine()
      try {
        val report = diagnostic.runAliceBobTest()
        Arguments.createMap().apply {
          putString("openmlsVersion", report.openmlsVersion); putBoolean("aliceToBob", report.aliceToBob)
          putBoolean("bobToAlice", report.bobToAlice); putBoolean("tamperRejected", report.tamperRejected)
          putBoolean("commitProcessed", report.commitProcessed); putBoolean("snapshotRestored", report.snapshotRestored)
        }
      } finally { diagnostic.closeEngine(); diagnostic.destroy() }
    }
  }
  override fun invalidate() { storage.releaseModule(); super.invalidate() }
}
