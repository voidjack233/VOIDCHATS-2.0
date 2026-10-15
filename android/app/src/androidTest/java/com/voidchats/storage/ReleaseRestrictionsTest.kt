package com.voidchats.storage

import android.content.pm.ApplicationInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.BridgeReactContext
import com.voidchats.crypto.VoidOpenMlsModule
import java.lang.reflect.Proxy
import java.util.UUID
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ReleaseRestrictionsTest {
  @Test fun installedReleaseRejectsDevelopmentIdentityAndSelfTestWithoutCreatingKeys() {
    assumeTrue(InstrumentationRegistry.getArguments().getString("releaseRestrictions") == "true")
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    assertEquals(0, context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE)
    val namespace = "instrumentation-release-${UUID.randomUUID()}"
    val storage = AppStorage(context, namespace)
    try {
      // Calls target APK bytecode; never inspect the test APK's inlined BuildConfig.DEBUG.
      try { storage.initializeDevelopment(); fail("Release must reject local development creation") }
      catch (error: StorageFailure) { assertEquals("DEVELOPMENT_ONLY", error.code) }
      assertFalse(EncryptedDatabase(context, namespace).exists())
      var rejection: String? = null; var resolved = false
      val promise = Proxy.newProxyInstance(Promise::class.java.classLoader, arrayOf(Promise::class.java)) { _, method, args ->
        if (method.name == "reject") rejection = args?.firstOrNull() as? String
        if (method.name == "resolve") resolved = true
        null
      } as Promise
      val module = VoidOpenMlsModule(BridgeReactContext(context), storage)
      module.runSelfTest(promise)
      assertEquals("MLS_UNAVAILABLE", rejection); assertFalse(resolved)
      assertFalse(EncryptedDatabase(context, namespace).exists())
      module.invalidate()
    } finally { storage.removeLocalData(); storage.releaseModule() }
  }
}
