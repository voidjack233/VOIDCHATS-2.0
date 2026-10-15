package com.voidchats.storage

import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod
import org.json.JSONObject
import org.json.JSONTokener
import org.json.JSONException

internal class VoidStorageModule(context: ReactApplicationContext, private val storage: AppStorage) : ReactContextBaseJavaModule(context) {
  override fun getName() = "VoidStorage"
  @ReactMethod fun getStatus(promise: Promise) = storage.submit(promise) { storage.status() }
  @ReactMethod fun open(promise: Promise) = storage.submit(promise) { storage.open() }
  @ReactMethod fun initializeDevelopment(promise: Promise) = storage.submit(promise) { storage.initializeDevelopment() }
  @ReactMethod fun close(promise: Promise) = storage.submit(promise) { storage.lock() }
  @ReactMethod fun removeLocalData(promise: Promise) = storage.submit(promise) { storage.removeLocalData() }
  @ReactMethod fun getLocalIdentity(promise: Promise) = storage.submit(promise) { storage.localIdentity()?.toString() ?: "null" }
  @ReactMethod fun execute(operation: String, payloadJSON: String, promise: Promise) {
    if (operation.length > 80 || payloadJSON.length > 1_100_000) { promise.reject("INVALID_INPUT", "The local storage input exceeds the supported size."); return }
    storage.submit(promise) {
      try {
        val parser = JSONTokener(payloadJSON)
        val payload = parser.nextValue() as? JSONObject ?: throw IllegalArgumentException()
        require(parser.nextClean() == '\u0000')
        storage.execute(operation, payload)
      } catch (_: JSONException) { throw IllegalArgumentException() }
    }
  }
  override fun invalidate() { storage.releaseModule(); super.invalidate() }
}
