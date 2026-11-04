package com.voidchats.storage

import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod

/** Public transport artifacts only; private Signal keys never cross this bridge. */
internal class VoidMessagingModule(context: ReactApplicationContext, private val storage: AppStorage) : ReactContextBaseJavaModule(context) {
  private val messaging = SignalMessagingStore(storage)
  override fun getName() = "VoidMessaging"
  @ReactMethod fun signAuthChallenge(messageBase64: String, promise: Promise) = storage.submit(promise) { messaging.signAuthChallenge(messageBase64) }
  @ReactMethod fun getState(promise: Promise) = storage.submit(promise) { messaging.state().toString() }
  @ReactMethod fun logSendDiagnostics(conversationId: String, promise: Promise) = storage.submit(promise) { messaging.logSendDiagnostics(conversationId).toString() }
  @ReactMethod fun resumePending(promise: Promise) = storage.submit(promise) { messaging.resumePending().toString() }
  @ReactMethod fun getPreKeyPublication(promise: Promise) = storage.submit(promise) { messaging.getPreKeyPublication().toString() }
  @ReactMethod fun completePreKeyPublication(publicationId: String, promise: Promise) = storage.submit(promise) { messaging.completePreKeyPublication(publicationId).toString() }
  @ReactMethod fun establishSignalSession(peerJSON: String, bundleBase64: String, promise: Promise) = storage.submit(promise) {
    messaging.establishSignalSession(MessagingProtocol.objectValue(MessagingProtocol.parse(peerJSON, 4096)), bundleBase64).toString()
  }
  @ReactMethod fun openConversation(peerJSON: String, promise: Promise) = storage.submit(promise) { messaging.openConversation(MessagingProtocol.objectValue(MessagingProtocol.parse(peerJSON, 4096))).toString() }
  @ReactMethod fun blockPeer(peerVoidId: String, promise: Promise) = storage.submit(promise) { messaging.blockPeer(peerVoidId).toString() }
  @ReactMethod fun queueMessage(conversationId: String, body: String, promise: Promise) = storage.submit(promise) { messaging.queueMessage(conversationId, body).toString() }
  @ReactMethod fun completeOperation(operationId: String, responseJSON: String, promise: Promise) = storage.submit(promise) { messaging.completeOperation(operationId, MessagingProtocol.parse(responseJSON)).toString() }
  @ReactMethod fun failOperation(operationId: String, errorCode: String, promise: Promise) = storage.submit(promise) { messaging.failOperation(operationId, errorCode).toString() }
  @ReactMethod fun failPending(errorCode: String, peerVoidId: String?, promise: Promise) = storage.submit(promise) { messaging.failPending(errorCode, peerVoidId).toString() }
  @ReactMethod fun retryOperation(operationId: String, promise: Promise) = storage.submit(promise) { messaging.retryOperation(operationId).toString() }
  @ReactMethod fun applyOnline(senderVoidId: String, exchangeId: String, payloadBase64: String, peerJSON: String, promise: Promise) = storage.submit(promise) {
    throw SignalFailure("SIGNAL_UNSUPPORTED_MESSAGE")
  }
  @ReactMethod fun applyEvent(eventJSON: String, peerJSON: String, promise: Promise) = storage.submit(promise) { messaging.applyEvent(MessagingProtocol.objectValue(MessagingProtocol.parse(eventJSON)), MessagingProtocol.objectValue(MessagingProtocol.parse(peerJSON, 4096))).toString() }
  override fun invalidate() { storage.releaseModule(); super.invalidate() }
}
