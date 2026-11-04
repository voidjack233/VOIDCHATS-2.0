package com.voidchats.storage

import android.util.Log
import java.util.EnumSet
import org.signal.libsignal.protocol.InvalidMessageException
import org.signal.libsignal.protocol.logging.SignalProtocolLogger
import org.signal.libsignal.protocol.logging.SignalProtocolLoggerProvider

/** Operation-local reason flags only; native log strings never leave the callback. */
internal object SignalDecryptDiagnostics {
  private enum class Reason {
    MAC_MISMATCH, PQ_RATCHET_ERROR, CIPHERTEXT_DECRYPT_ERROR, FUTURE_COUNTER,
    MESSAGE_VERSION, INVALID_SESSION, NO_CURRENT_SESSION, CORRUPT_ARCHIVED_SESSION,
    OTHER_CANDIDATE_FAILURE
  }
  private val active = ThreadLocal<EnumSet<Reason>>()
  private var attemptedInitialization = false
  private val source = Regex("^(?:.*[/\\\\])?(?:rust[/\\\\]protocol[/\\\\])?src[/\\\\]session_management\\.rs:[0-9]+: ")
  private val provider = SignalProtocolLogger { priority, tag, message ->
    val reasons = active.get()
    if (reasons != null && priority in SignalProtocolLogger.WARN..SignalProtocolLogger.ERROR && tag == "libsignal" && message != null && message.length <= 65536) {
      try { collect(message, reasons) } catch (_: RuntimeException) { /* Diagnostics cannot affect decryption. */ }
    }
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

  fun <T> capture(operation: () -> T): T {
    initialize()
    // Never replace a caller's context if capture is unexpectedly nested.
    if (active.get() != null) return operation()
    val reasons = EnumSet.noneOf(Reason::class.java)
    active.set(reasons)
    try { return operation() }
    catch (error: InvalidMessageException) {
      val flags = if (reasons.isEmpty()) "UNKNOWN" else reasons.joinToString(",") { it.name }
      try {
        Log.w("VoidMessaging", "stage=message_decrypt code=SIGNAL_MESSAGE_DECRYPT_INVALID_MESSAGE candidate_reasons=$flags")
      } catch (_: RuntimeException) { /* Preserve the original native exception. */ }
      throw error
    } finally { active.remove() }
  }
}
