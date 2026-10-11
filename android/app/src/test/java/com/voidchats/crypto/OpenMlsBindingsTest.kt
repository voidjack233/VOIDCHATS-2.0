package com.voidchats.crypto

import com.voidchats.openmls.MlsEngine
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Host JVM verification of the generated Kotlin -> JNA -> real Rust boundary. */
class OpenMlsBindingsTest {
  @Test
  fun aliceAndBobExchangeAcrossGeneratedBindings() {
    val engine = MlsEngine()
    try {
      assertEquals("0.9.1", engine.engineInfo().openmlsVersion)
      assertFalse(engine.engineInfo().durablePersistence)
      val alice = engine.createClient("bridge-test-alice".toByteArray())
      val bob = engine.createClient("bridge-test-bob".toByteArray())
      assertEquals(32, alice.signaturePublicKey.size)
      val groupId = "bridge-test-group".toByteArray()
      assertEquals(0UL, engine.createGroup(alice.clientHandle, groupId).epoch)
      val addition = engine.addMember(alice.clientHandle, groupId, engine.generateKeyPackage(bob.clientHandle))
      assertEquals(1UL, engine.mergePendingCommit(alice.clientHandle, groupId).epoch)
      assertEquals(2U, engine.processWelcome(bob.clientHandle, addition.welcome).memberCount)
      val message = "Authenticated bridge message".toByteArray()
      val received = engine.decrypt(bob.clientHandle, groupId, engine.encrypt(alice.clientHandle, groupId, message))
      assertArrayEquals(message, received.plaintext)
      assertArrayEquals(alice.credentialIdentity, received.senderIdentity)
      assertArrayEquals(alice.signaturePublicKey, received.senderSignaturePublicKey)
      val reply = "Authenticated bridge reply".toByteArray()
      assertArrayEquals(reply, engine.decrypt(alice.clientHandle, groupId, engine.encrypt(bob.clientHandle, groupId, reply)).plaintext)
      val snapshot = engine.suspendClient(bob.clientHandle)
      val restored = engine.restoreClient(snapshot)
      assertArrayEquals(bob.signaturePublicKey, restored.signaturePublicKey)
      assertEquals(1UL, engine.groupInfo(restored.clientHandle, groupId).epoch)
      engine.releaseClient(alice.clientHandle)
      engine.releaseClient(restored.clientHandle)
    } finally {
      engine.destroy()
    }
  }

  @Test
  fun rustSelfTestReportCrossesGeneratedBindings() {
    val engine = MlsEngine()
    try {
      val report = engine.runAliceBobTest()
      assertTrue(report.aliceToBob)
      assertTrue(report.bobToAlice)
      assertTrue(report.tamperRejected)
      assertTrue(report.commitProcessed)
      assertTrue(report.snapshotRestored)
    } finally {
      engine.destroy()
    }
  }
}
