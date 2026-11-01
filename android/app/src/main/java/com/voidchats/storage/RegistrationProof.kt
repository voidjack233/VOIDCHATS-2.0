package com.voidchats.storage

import android.util.Base64
import com.voidchats.BuildConfig
import java.net.URI
import java.text.ParsePosition
import java.text.SimpleDateFormat
import java.util.Locale
import java.util.TimeZone

/** Protocol framing only. Actual Ed25519 signing stays in protected Rust. */
internal object RegistrationProof {
  private val opaqueToken = Regex("^[A-Za-z0-9_-]{43}$")
  private val timestamp = Regex("^([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\\.([0-9]{1,9}))?(Z|[+-][0-9]{2}:[0-9]{2})$")

  fun validUsername(value: String): Boolean {
    if (value.isEmpty() || value.codePointCount(0, value.length) !in 1..32) return false
    val points = value.codePoints().toArray()
    if (points.any { Character.getType(it) == Character.CONTROL.toInt() || Character.getType(it) == Character.FORMAT.toInt() || it in 0xD800..0xDFFF }) return false
    // Matches Go unicode.IsSpace, including NEL (which Java whitespace omits).
    fun space(point: Int) = Character.isWhitespace(point) || Character.isSpaceChar(point) || point == 0x85
    return !space(points.first()) && !space(points.last())
  }

  fun validateApiUrl(value: String) {
    require(value.length in 1..2048 && !value.endsWith('/'))
    val uri = try { URI(value) } catch (_: java.net.URISyntaxException) { throw IllegalArgumentException() }
    require(uri.host != null && uri.rawUserInfo == null && uri.rawQuery == null && uri.rawFragment == null)
    require(uri.rawPath.isNullOrEmpty() || (uri.rawPath.startsWith('/') && !uri.rawPath.endsWith('/') && !uri.rawPath.contains("..") && !uri.rawPath.contains('%')))
    require(uri.scheme == "https" || (BuildConfig.DEBUG && uri.scheme == "http" && uri.host in setOf("127.0.0.1", "localhost", "10.0.2.2", "[::1]")))
  }

  fun timestampMillis(value: String): Long {
    val match = timestamp.matchEntire(value) ?: throw IllegalArgumentException()
    val zone = match.groupValues[3]
    if (zone != "Z") { require(zone.substring(1, 3).toInt() <= 23 && zone.substring(4, 6).toInt() <= 59) }
    val parser = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss", Locale.US).apply {
      isLenient = false
      timeZone = TimeZone.getTimeZone(if (zone == "Z") "UTC" else "GMT$zone")
    }
    val cursor = ParsePosition(0)
    val date = parser.parse(match.groupValues[1], cursor) ?: throw IllegalArgumentException()
    require(cursor.index == match.groupValues[1].length)
    return date.time + match.groupValues[2].padEnd(3, '0').take(3).toLong()
  }

  fun bytes(value: String): ByteArray {
    require(value.length in 1..5500)
    return Base64.decode(value, Base64.NO_WRAP).also { require(base64(it) == value) }
  }

  fun base64(value: ByteArray): String = Base64.encodeToString(value, Base64.NO_WRAP)

  fun validate(messageBase64: String, username: String, publicKeyBase64: String, now: Long = System.currentTimeMillis()): ByteArray {
    val message = bytes(messageBase64)
    require(message.size <= 4096)
    val text = message.toString(Charsets.UTF_8)
    require(text.toByteArray(Charsets.UTF_8).contentEquals(message))
    val fields = text.split('\n')
    require(fields.size == 7 && fields[0] == "VOID-NOTHINGNESS:register:v1" && fields[6].isEmpty())
    for (index in listOf(1, 5)) {
      require(opaqueToken.matches(fields[index]))
      val decoded = Base64.decode(fields[index], Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
      require(decoded.size == 32 && Base64.encodeToString(decoded, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING) == fields[index])
    }
    val expiry = timestampMillis(fields[2])
    require(expiry > now && expiry - now <= 10 * 60 * 1000L + 30_000)
    require(fields[3] == publicKeyBase64 && fields[4] == base64(username.toByteArray(Charsets.UTF_8)))
    return message
  }
}
