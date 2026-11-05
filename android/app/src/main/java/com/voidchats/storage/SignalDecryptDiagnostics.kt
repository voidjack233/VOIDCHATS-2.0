package com.voidchats.storage

import android.util.Log
import java.security.MessageDigest
import java.util.EnumSet
import org.json.JSONObject
import org.signal.libsignal.protocol.InvalidMessageException
import org.signal.libsignal.protocol.logging.SignalProtocolLogger
import org.signal.libsignal.protocol.logging.SignalProtocolLoggerProvider
import org.signal.libsignal.protocol.message.CiphertextMessage
import org.signal.libsignal.protocol.message.PreKeySignalMessage
import org.signal.libsignal.protocol.message.SignalMessage

/** Operation-local reason flags only; native log strings never leave the callback. */
internal object SignalDecryptDiagnostics {
  enum class DecryptStage(val label: String, val code: String) {
    PREKEY("prekey_decrypt", "SIGNAL_PREKEY_DECRYPT_INVALID_MESSAGE"),
    MESSAGE("message_decrypt", "SIGNAL_MESSAGE_DECRYPT_INVALID_MESSAGE")
  }
  enum class CommitStage(val label: String) {
    ENCRYPT("encrypt_committed"), RECEIVE("receive_committed"), DUPLICATE("receive_duplicate_committed")
  }
  private enum class Reason {
    MAC_MISMATCH, PQ_RATCHET_ERROR, CIPHERTEXT_DECRYPT_ERROR, FUTURE_COUNTER,
    MESSAGE_VERSION, INVALID_SESSION, NO_CURRENT_SESSION, CORRUPT_ARCHIVED_SESSION,
    OTHER_CANDIDATE_FAILURE
  }
  private val active = ThreadLocal<EnumSet<Reason>>()
  private var attemptedInitialization = false
  private val source = Regex("^(?:.*[/\\\\])?(?:rust[/\\\\]protocol[/\\\\])?src[/\\\\]session_management\\.rs:[0-9]+: ")
  private val provider = SignalProtocolLogger { priority, tag, message ->
    bestEffort {
      val reasons = active.get()
      if (reasons != null && priority in SignalProtocolLogger.WARN..SignalProtocolLogger.ERROR && tag == "libsignal" && message != null && message.length <= 65536) {
        collect(message, reasons)
      }
    }
  }

  private inline fun bestEffort(operation: () -> Unit) {
    try { operation() } catch (_: Throwable) { /* Optional diagnostics must not alter a native result. */ }
  }

  @Synchronized private fun initialize() {
    if (attemptedInitialization) return
    attemptedInitialization = true
    try {
      // A previously configured provider belongs to its owner.
      if (SignalProtocolLoggerProvider.getProvider() != null) return
      SignalProtocolLoggerProvider.setProvider(provider)
      try { SignalProtocolLoggerProvider.initializeLogging(SignalProtocolLogger.WARN) }
      catch (_: RuntimeException) { removeProvider() }
      catch (_: LinkageError) { removeProvider() }
      catch (_: AssertionError) { removeProvider() }
    } catch (_: RuntimeException) { /* Keep the native operation authoritative. */ }
    catch (_: LinkageError) { /* Missing diagnostic support is optional. */ }
  }

  private fun removeProvider() {
    try {
      if (SignalProtocolLoggerProvider.getProvider() === provider) SignalProtocolLoggerProvider.setProvider(null)
    } catch (_: RuntimeException) { }
    catch (_: LinkageError) { }
  }

  private fun collect(message: String, reasons: EnumSet<Reason>) {
    val prefix = source.find(message) ?: return
    val body = message.substring(prefix.range.last + 1)
    if (body.startsWith("Skipping corrupt archived session ")) {
      reasons.add(Reason.CORRUPT_ARCHIVED_SESSION)
    } else if (body.startsWith("Failed to decrypt Whisper message with ratchet key: ") && body.contains(" session has base key: ")) {
      reasons.add(candidateReason(body))
    } else if (body.startsWith("Message from ") && body.contains(" failed to decrypt; sender ratchet public key ")) {
      for (line in body.lineSequence()) {
        if (line == "No current session") reasons.add(Reason.NO_CURRENT_SESSION)
        else if (line.startsWith("Candidate session ") && line.contains(" failed with '")) reasons.add(candidateReason(line))
      }
    }
  }

  private fun candidateReason(line: String): Reason = when {
    line.contains("MAC verification failed") -> Reason.MAC_MISMATCH
    line.contains("post-quantum ratchet error: ") -> Reason.PQ_RATCHET_ERROR
    line.contains("invalid Whisper message: failed to decrypt: ") -> Reason.CIPHERTEXT_DECRYPT_ERROR
    line.contains("message from too far into the future") -> Reason.FUTURE_COUNTER
    line.contains("unrecognized message version <") -> Reason.MESSAGE_VERSION
    line.contains("invalid session: ") -> Reason.INVALID_SESSION
    else -> Reason.OTHER_CANDIDATE_FAILURE
  }

  private fun eventMetadata(eventId: String, payloadBase64: String): String {
    val safeId = try { MessagingProtocol.uuid(eventId) } catch (_: IllegalArgumentException) { "UNKNOWN" }
    var sender = "UNKNOWN"
    var recipient = "UNKNOWN"
    var payloadHash = "UNKNOWN"
    var ciphertextHash = "UNKNOWN"
    var messageType = "UNKNOWN"
    var ratchetHash = "UNKNOWN"
    var counter = "UNKNOWN"
    var version = "UNKNOWN"
    fun hash(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it.toInt() and 255) }
    try {
      val payload = MessagingProtocol.bytes(payloadBase64)
      try {
        payloadHash = hash(payload)
        val envelope = JSONObject(payload.toString(Charsets.UTF_8))
        try { sender = MessagingProtocol.voidId(MessagingProtocol.text(envelope, "from_void_id", 19)) } catch (_: IllegalArgumentException) { }
        try { recipient = MessagingProtocol.voidId(MessagingProtocol.text(envelope, "to_void_id", 19)) } catch (_: IllegalArgumentException) { }
        val ciphertext = MessagingProtocol.bytes(MessagingProtocol.text(envelope, "ciphertext_base64", 1_398_104))
        try {
          ciphertextHash = hash(ciphertext)
          val message = when (envelope.optInt("message_type", -1)) {
            CiphertextMessage.PREKEY_TYPE -> {
              messageType = "PREKEY"
              PreKeySignalMessage(ciphertext).whisperMessage
            }
            CiphertextMessage.WHISPER_TYPE -> {
              messageType = "WHISPER"
              SignalMessage(ciphertext)
            }
            else -> null
          }
          if (message != null) {
            counter = message.counter.toString()
            version = message.messageVersion.toString()
            val publicRatchet = message.senderRatchetKey.serialize()
            try { ratchetHash = hash(publicRatchet) } finally { publicRatchet.fill(0) }
          }
        } finally { ciphertext.fill(0) }
      } finally { payload.fill(0) }
    } catch (_: Throwable) { /* Unavailable diagnostic metadata stays UNKNOWN. */ }
    return "event_id=$safeId sender_void_id=$sender recipient_void_id=$recipient payload_sha256=$payloadHash ciphertext_sha256=$ciphertextHash message_type=$messageType sender_ratchet_sha256=$ratchetHash message_counter=$counter message_version=$version"
  }

  /** Explicit, read-only inspection of an already saved outgoing event. */
  fun logStoredSend(eventId: String, payloadBase64: String, createdAt: Long) {
    bestEffort {
      Log.w("VoidMessaging", "stage=stored_send ${eventMetadata(eventId, payloadBase64)} message_created_at=$createdAt draft_created_at_ms=$createdAt")
    }
  }

  /** Call only after the enclosing transaction has returned and no outer transaction remains. */
  fun logCommittedEvent(stage: CommitStage, localVoidId: String, eventId: String, payloadBase64: String,
    draftCreatedAt: Long? = null, encryptedAt: Long? = null) = bestEffort {
    val committedAt = System.currentTimeMillis()
    val local = MessagingProtocol.voidId(localVoidId)
    val timing = (draftCreatedAt?.let { " draft_created_at_ms=$it" } ?: "") +
      (encryptedAt?.let { " encrypted_at_ms=$it" } ?: "")
    Log.i("VoidMessaging", "stage=${stage.label} local_void_id=$local ${eventMetadata(eventId, payloadBase64)} committed_at_ms=$committedAt$timing")
  }

  /** Public session properties only; never serialize the record for diagnostics. */
  fun logSession(localVoidId: String, peerVoidId: String, created: Boolean, hasSenderChain: Boolean, version: Int) = bestEffort {
    val local = MessagingProtocol.voidId(localVoidId)
    val peer = MessagingProtocol.voidId(peerVoidId)
    Log.i("VoidMessaging", "stage=session_committed local_void_id=$local peer_void_id=$peer created=$created has_sender_chain=$hasSenderChain session_version=$version committed_at_ms=${System.currentTimeMillis()}")
  }

  fun <T> capture(stage: DecryptStage, eventId: String, payloadBase64: String, operation: () -> T): T {
    val reasons = try {
      initialize()
      // Never replace a caller's context if capture is unexpectedly nested.
      if (active.get() == null) EnumSet.noneOf(Reason::class.java).also { active.set(it) } else null
    } catch (_: Throwable) { null }
    if (reasons == null) return operation()
    try { return operation() }
    catch (error: InvalidMessageException) {
      bestEffort {
        val flags = if (reasons.isEmpty()) "UNKNOWN" else reasons.joinToString(",") { it.name }
        Log.w("VoidMessaging", "stage=${stage.label} code=${stage.code} ${eventMetadata(eventId, payloadBase64)} candidate_reasons=$flags")
      }
      throw error
    } finally { bestEffort { active.remove() } }
  }
}
