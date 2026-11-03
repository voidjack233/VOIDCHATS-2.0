package com.voidchats.storage

import android.util.Base64
import java.security.MessageDigest
import java.util.UUID
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener

internal object MessagingProtocol {
  fun parse(text: String, maximum: Int = 4 * 1024 * 1024): Any {
    require(text.length <= maximum)
    try {
      val parser = JSONTokener(text)
      val value = parser.nextValue()
      require(parser.nextClean() == '\u0000')
      return value
    } catch (_: org.json.JSONException) { throw IllegalArgumentException() }
  }
  fun objectValue(value: Any?): JSONObject = value as? JSONObject ?: throw IllegalArgumentException()
  fun text(value: JSONObject, key: String, maximum: Int = 4096): String {
    val result = value.opt(key) as? String ?: throw IllegalArgumentException()
    require(result.length <= maximum && !result.contains('\u0000'))
    return result
  }
  fun uuid(value: String): String {
    require(value.length == 36 && value != "00000000-0000-0000-0000-000000000000" && UUID.fromString(value).toString() == value)
    return value
  }
  fun voidId(value: String): String {
    require(Regex("[1-9][0-9]{0,18}").matches(value) && value.toLongOrNull() != null)
    return value
  }
  fun bytes(value: String, maximum: Int = 1024 * 1024): ByteArray {
    require(value.isNotEmpty() && value.length <= (maximum + 2) / 3 * 4)
    return Base64.decode(value, Base64.NO_WRAP).also { require(it.size in 1..maximum && base64(it) == value) }
  }
  fun base64(value: ByteArray): String = Base64.encodeToString(value, Base64.NO_WRAP)
  fun canonical(value: Any?): String = when (value) {
    is JSONObject -> value.keys().asSequence().toList().sorted().joinToString(",", "{", "}") { JSONObject.quote(it) + ":" + canonical(value.get(it)) }
    is JSONArray -> (0 until value.length()).joinToString(",", "[", "]") { canonical(value.get(it)) }
    is String -> JSONObject.quote(value)
    null, JSONObject.NULL -> "null"
    else -> value.toString()
  }
  fun hash(value: String): String = MessageDigest.getInstance("SHA-256").digest(value.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it.toInt() and 255) }
  fun person(value: Any?): JSONObject = objectValue(value).also {
    voidId(text(it, "void_id", 19))
    require(bytes(text(it, "auth_public_key", 44), 32).size == 32)
  }
  fun authMessage(encoded: String, identity: JSONObject, now: Long = System.currentTimeMillis()): ByteArray {
    val message = bytes(encoded, 4096)
    val decoded = message.toString(Charsets.UTF_8)
    require(decoded.toByteArray(Charsets.UTF_8).contentEquals(message))
    val fields = decoded.split('\n')
    require(fields.size == 8 && fields[0] == "VOID-NOTHINGNESS:authenticate:v2" && fields[7].isEmpty())
    for (index in listOf(1, 6)) {
      require(Regex("[A-Za-z0-9_-]{43}").matches(fields[index]))
      val token = Base64.decode(fields[index], Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
      require(token.size == 32 && Base64.encodeToString(token, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING) == fields[index])
    }
    val expiry = RegistrationProof.timestampMillis(fields[4])
    require(expiry > now && expiry - now <= 30_000)
    require(fields[2] == identity.getString("accountVoidId") && fields[3] == identity.getString("signaturePublicKeyBase64"))
    require(fields[5] == base64(identity.getString("accountApiUrl").toByteArray(Charsets.UTF_8)))
    return message
  }
}
