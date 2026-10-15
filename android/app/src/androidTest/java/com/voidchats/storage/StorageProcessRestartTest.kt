package com.voidchats.storage

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.voidchats.openmls.MlsException
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.Assume.assumeTrue
import org.junit.runner.RunWith

/** Run seed and verify as separate instrumentation invocations, force-stopping between them. */
@RunWith(AndroidJUnit4::class)
class StorageProcessRestartTest {
  @Test fun encryptedMlsAndHistorySurviveRealProcessRestart() {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val alice = AppStorage(context, "instrumentation-process-alice")
    val bob = AppStorage(context, "instrumentation-process-bob")
    val group = "process-restart-group".toByteArray()
    val phase = InstrumentationRegistry.getArguments().getString("phase")
    assumeTrue(phase == "seed" || phase == "crash" || phase == "verify")
    InstrumentationRegistry.getInstrumentation().sendStatus(0, android.os.Bundle().apply {
      putString("storagePhase", phase); putString("processPid", android.os.Process.myPid().toString())
    })
    try {
      if (phase == "seed") {
        alice.removeLocalData(); bob.removeLocalData()
        alice.initializeDevelopment(); bob.initializeDevelopment()
        val a = alice.localIdentity()!!.getString("clientHandle"); val b = bob.localIdentity()!!.getString("clientHandle")
        alice.mutateMls { it.createGroup(a, group) }
        val keyPackage = bob.mutateMls { it.generateKeyPackage(b) }
        val added = alice.mutateMls { it.addMember(a, group, keyPackage) }
        alice.mutateMls { it.mergePendingCommit(a, group) }; bob.mutateMls { it.processWelcome(b, added.welcome) }
        val plaintext = "private payload before process restart".toByteArray()
        val wire = alice.mutateMls { val output = it.encrypt(a, group, plaintext); alice.journal("encrypt", group, plaintext, output); output }
        bob.mutateMls {
          val result = it.decrypt(b, group, wire)
          bob.journal("decrypt", group, result.plaintext, wire, result.senderIdentity, result.senderSignaturePublicKey, result.epoch.toString())
          assertArrayEquals(plaintext, result.plaintext)
        }
        alice.execute("conversations.save", JSONObject("""{"id":"retained-chat","kind":"group","title":"Process test","peerVoidId":null,"mlsGroupId":null,"createdAt":1,"updatedAt":1,"members":[]}"""))
        alice.execute("messages.save", JSONObject("""{"id":"retained-message","conversationId":"retained-chat","senderVoidId":"local-test-sender","body":"Retained encrypted history","direction":"outgoing","deliveryState":"pending","createdAt":1}"""))
        alice.requireDatabase().execSQL("CREATE TABLE instrumentation_fault_checkpoint(revision INTEGER NOT NULL,event_count INTEGER NOT NULL)")
        alice.requireDatabase().execSQL("INSERT INTO instrumentation_fault_checkpoint SELECT revision,(SELECT count(*) FROM crypto_events) FROM native_mls_state WHERE slot=1")
      } else if (phase == "crash") {
        alice.open()
        val a = alice.localIdentity()!!.getString("clientHandle")
        for (pragma in listOf("cache_size=1", "cache_spill=ON")) alice.requireDatabase().rawQuery("PRAGMA $pragma", emptyArray()).use { while (it.moveToNext()) { } }
        alice.mutateMls {
          val plaintext = "never committed or returned".toByteArray()
          val wire = it.encrypt(a, group, plaintext)
          alice.journal("encrypt", group, plaintext, wire)
          assertTrue(java.io.File(context.noBackupFilesDir, "instrumentation-process-alice/store.db-journal").length() > 0)
          android.os.Process.killProcess(android.os.Process.myPid())
          throw AssertionError("Crash phase must terminate before transaction completion")
        }
      } else if (phase == "verify") {
        alice.open(); bob.open()
        alice.requireDatabase().rawQuery("SELECT revision,event_count,(SELECT revision FROM native_mls_state WHERE slot=1),(SELECT count(*) FROM crypto_events) FROM instrumentation_fault_checkpoint", emptyArray()).use {
          assertTrue(it.moveToFirst()); assertEquals(it.getLong(0), it.getLong(2)); assertEquals(it.getLong(1), it.getLong(3))
        }
        val a = alice.localIdentity()!!.getString("clientHandle"); val b = bob.localIdentity()!!.getString("clientHandle")
        assertEquals("Retained encrypted history", JSONObject(alice.execute("messages.get", JSONObject().put("id", "retained-message"))).getString("body"))
        val wire = alice.requireDatabase().rawQuery("SELECT ciphertext FROM crypto_events WHERE operation='encrypt'", emptyArray()).use { assertTrue(it.moveToFirst()); it.getBlob(0) }
        bob.requireDatabase().rawQuery("SELECT plaintext,sender_identity,sender_signature_public_key,epoch,checkpoint_revision FROM crypto_events WHERE operation='decrypt'", emptyArray()).use {
          assertTrue(it.moveToFirst()); assertEquals("private payload before process restart", it.getBlob(0).toString(Charsets.UTF_8)); assertTrue(it.getBlob(1).isNotEmpty()); assertEquals(32, it.getBlob(2).size); assertTrue(it.getString(3).matches(Regex("[0-9]+"))); assertTrue(it.getLong(4) > 0)
        }
        val recovered = JSONObject(bob.readStoredDecryptions("null", 1.0))
        assertEquals(1, recovered.getJSONArray("items").length()); assertTrue(recovered.isNull("nextCursor"))
        val recoveredItem = recovered.getJSONArray("items").getJSONObject(0)
        assertEquals("private payload before process restart", android.util.Base64.decode(recoveredItem.getString("plaintextBase64"), android.util.Base64.NO_WRAP).toString(Charsets.UTF_8))
        val emptyPage = JSONObject(bob.readStoredDecryptions(JSONObject().put("createdAt", recoveredItem.getLong("createdAt")).put("id", recoveredItem.getString("eventId")).toString(), 1.0))
        assertEquals(0, emptyPage.getJSONArray("items").length())
        val rejectedRevisionBefore = bob.requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use { assertTrue(it.moveToFirst()); it.getLong(0) }
        try { bob.mutateMls { it.decrypt(b, group, wire) }; fail("Consumed ciphertext must remain rejected after restart") } catch (_: MlsException.InvalidMessage) { }
        val rejectedRevisionAfter = bob.requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use { assertTrue(it.moveToFirst()); it.getLong(0) }
        assertTrue("A rejected operation still commits its ratchet checkpoint", rejectedRevisionAfter > rejectedRevisionBefore)
        val reply = bob.mutateMls { it.encrypt(b, group, "reply after restart".toByteArray()) }
        assertEquals("reply after restart", alice.mutateMls { it.decrypt(a, group, reply) }.plaintext.toString(Charsets.UTF_8))
        val before = alice.requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use { assertTrue(it.moveToFirst()); it.getLong(0) }
        alice.requireDatabase().execSQL("CREATE TRIGGER fail_test_journal BEFORE INSERT ON crypto_events BEGIN SELECT RAISE(ABORT,'test_journal_failure'); END")
        var escaped = false
        try {
          alice.mutateMls { val output = it.encrypt(a, group, "withheld".toByteArray()); alice.journal("encrypt", group, "withheld".toByteArray(), output); output }
          escaped = true
        } catch (_: Exception) { }
        assertFalse(escaped); assertEquals("error", JSONObject(alice.status()).getString("status"))
        alice.open()
        val after = alice.requireDatabase().rawQuery("SELECT revision FROM native_mls_state WHERE slot=1", emptyArray()).use { assertTrue(it.moveToFirst()); it.getLong(0) }
        assertEquals(before, after)
        alice.requireDatabase().execSQL("DROP TRIGGER fail_test_journal")
      } else throw IllegalArgumentException("Run with instrumentation phase=seed, phase=crash or phase=verify")
    } finally {
      if (phase == "verify") { alice.removeLocalData(); bob.removeLocalData() } else { alice.lock(); bob.lock() }
    }
  }
}
