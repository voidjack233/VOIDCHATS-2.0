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
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

/** Thin transport adapter. All MLS state, private keys, and cryptography stay in Rust. */
class VoidOpenMlsModule(context: ReactApplicationContext) : ReactContextBaseJavaModule(context) {
  private var engine: MlsEngine? = null
  // Serial MLS state changes and a bounded queue prevent unlimited queued plaintext copies.
  private val executor = object : ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(32)) {
    override fun terminated() {
      engine?.destroy()
      engine = null
      super.terminated()
    }
  }
  @Volatile private var closed = false

  override fun getName() = "VoidOpenMls"

  private fun call(promise: Promise, vararg inputs: String, operation: (MlsEngine) -> Any?) {
    if (inputs.any { it.length > 1_398_104 }) {
      promise.reject("MLS_INVALID_INPUT", "The MLS input exceeds the supported size.")
      return
    }
    if (closed) {
      promise.reject("MLS_UNAVAILABLE", "The native MLS engine has been released.")
      return
    }
    try {
      executor.execute {
        try {
          val active = engine ?: MlsEngine().also { engine = it }
          promise.resolve(operation(active))
        } catch (_: LinkageError) {
          promise.reject("MLS_UNAVAILABLE", "The Rust MLS library could not be loaded. Rebuild the custom Android application.")
        } catch (_: IllegalArgumentException) {
          promise.reject("MLS_INVALID_INPUT", "The MLS input is invalid.")
        } catch (error: Exception) {
          // Rust exports fixed error categories. Do not forward keys, inputs, or debug traces.
          val category = error.javaClass.simpleName.replace(Regex("([a-z])([A-Z])"), "$1_$2").uppercase()
          promise.reject("MLS_$category", "The native MLS operation failed.")
        }
      }
    } catch (_: RejectedExecutionException) {
      promise.reject("MLS_BUSY", "The native MLS engine cannot accept another operation.")
    }
  }

  private fun bytes(encoded: String): ByteArray {
    require(encoded.length <= 1_398_104)
    val decoded = Base64.decode(encoded, Base64.NO_WRAP)
    require(base64(decoded) == encoded) { "Expected canonical Base64" }
    return decoded
  }

  private fun base64(value: ByteArray): String = Base64.encodeToString(value, Base64.NO_WRAP)

  private fun client(value: ClientInfo): WritableMap = Arguments.createMap().apply {
    putString("clientHandle", value.clientHandle)
    putString("credentialIdentityBase64", base64(value.credentialIdentity))
    putString("signaturePublicKeyBase64", base64(value.signaturePublicKey))
  }

  private fun group(value: GroupInfo): WritableMap = Arguments.createMap().apply {
    putString("groupIdBase64", base64(value.groupId))
    putString("epoch", value.epoch.toString())
    putDouble("memberCount", value.memberCount.toDouble())
  }

  @ReactMethod
  fun initialize(promise: Promise) = call(promise) {
    val info = it.engineInfo()
    Arguments.createMap().apply {
      putString("openmlsVersion", info.openmlsVersion)
      putString("ciphersuite", info.ciphersuite)
      putString("protocolVersion", info.protocolVersion)
      putBoolean("durablePersistence", info.durablePersistence)
    }
  }

  @ReactMethod
  fun createClient(identityBase64: String, promise: Promise) = call(promise, identityBase64) {
    client(it.createClient(bytes(identityBase64)))
  }

  @ReactMethod
  fun generateKeyPackage(clientHandle: String, promise: Promise) = call(promise, clientHandle) {
    base64(it.generateKeyPackage(clientHandle))
  }

  @ReactMethod
  fun createGroup(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    group(it.createGroup(clientHandle, bytes(groupIdBase64)))
  }

  @ReactMethod
  fun addMember(clientHandle: String, groupIdBase64: String, keyPackageBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, keyPackageBase64) {
    val result = it.addMember(clientHandle, bytes(groupIdBase64), bytes(keyPackageBase64))
    Arguments.createMap().apply {
      putString("commitBase64", base64(result.commit))
      putString("welcomeBase64", base64(result.welcome))
    }
  }

  @ReactMethod
  fun mergePendingCommit(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    group(it.mergePendingCommit(clientHandle, bytes(groupIdBase64)))
  }

  @ReactMethod
  fun processWelcome(clientHandle: String, welcomeBase64: String, promise: Promise) = call(promise, clientHandle, welcomeBase64) {
    group(it.processWelcome(clientHandle, bytes(welcomeBase64)))
  }

  @ReactMethod
  fun encrypt(clientHandle: String, groupIdBase64: String, plaintextBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, plaintextBase64) {
    base64(it.encrypt(clientHandle, bytes(groupIdBase64), bytes(plaintextBase64)))
  }

  @ReactMethod
  fun decrypt(clientHandle: String, groupIdBase64: String, messageBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, messageBase64) {
    val result = it.decrypt(clientHandle, bytes(groupIdBase64), bytes(messageBase64))
    Arguments.createMap().apply {
      putString("plaintextBase64", base64(result.plaintext))
      putString("senderIdentityBase64", base64(result.senderIdentity))
      putString("senderSignaturePublicKeyBase64", base64(result.senderSignaturePublicKey))
      putString("epoch", result.epoch.toString())
    }
  }

  @ReactMethod
  fun selfUpdate(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    base64(it.selfUpdate(clientHandle, bytes(groupIdBase64)))
  }

  @ReactMethod
  fun processCommit(clientHandle: String, groupIdBase64: String, commitBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64, commitBase64) {
    group(it.processCommit(clientHandle, bytes(groupIdBase64), bytes(commitBase64)))
  }

  @ReactMethod
  fun groupInfo(clientHandle: String, groupIdBase64: String, promise: Promise) = call(promise, clientHandle, groupIdBase64) {
    group(it.groupInfo(clientHandle, bytes(groupIdBase64)))
  }

  @ReactMethod
  fun suspendClient(clientHandle: String, promise: Promise) = call(promise, clientHandle) {
    it.suspendClient(clientHandle)
  }

  @ReactMethod
  fun restoreClient(snapshotHandle: String, promise: Promise) = call(promise, snapshotHandle) {
    client(it.restoreClient(snapshotHandle))
  }

  @ReactMethod
  fun releaseClient(clientHandle: String, promise: Promise) = call(promise, clientHandle) {
    it.releaseClient(clientHandle)
    null
  }

  @ReactMethod
  fun discardSnapshot(snapshotHandle: String, promise: Promise) = call(promise, snapshotHandle) {
    it.discardSnapshot(snapshotHandle)
    null
  }

  @ReactMethod
  fun runSelfTest(promise: Promise) {
    if (!BuildConfig.DEBUG) {
      promise.reject("MLS_UNAVAILABLE", "The MLS self-test is available only in development builds.")
      return
    }
    call(promise) {
      val report = it.runAliceBobTest()
      Arguments.createMap().apply {
        putString("openmlsVersion", report.openmlsVersion)
        putBoolean("aliceToBob", report.aliceToBob)
        putBoolean("bobToAlice", report.bobToAlice)
        putBoolean("tamperRejected", report.tamperRejected)
        putBoolean("commitProcessed", report.commitProcessed)
        putBoolean("snapshotRestored", report.snapshotRestored)
      }
    }
  }

  @Synchronized
  override fun invalidate() {
    if (closed) return
    closed = true
    // Finish already accepted calls, then destroy the Rust object on termination.
    executor.shutdown()
    super.invalidate()
  }
}
