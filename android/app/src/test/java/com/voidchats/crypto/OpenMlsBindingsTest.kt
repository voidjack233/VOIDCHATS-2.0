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

  @Test
  fun accountBoundDirectOperationsCrossGeneratedBindings() {
    val engine = MlsEngine()
    try {
      val alice = engine.createClient("bound-bridge-alice".toByteArray())
      val bob = engine.createClient("bound-bridge-bob".toByteArray())
      val keyPackage = engine.generateKeyPackage(bob.clientHandle)
      assertArrayEquals(bob.credentialIdentity, engine.validateKeyPackage(keyPackage, bob.signaturePublicKey))
      val groupId = "bound-bridge-group".toByteArray()
      engine.createGroup(alice.clientHandle, groupId)
      val addition = engine.addMember(alice.clientHandle, groupId, keyPackage)
      engine.mergePendingCommit(alice.clientHandle, groupId)
      val joined = engine.processWelcomeChecked(bob.clientHandle, addition.welcome, groupId, alice.signaturePublicKey)
      assertArrayEquals(groupId, joined.group.groupId)
      assertArrayEquals(alice.credentialIdentity, joined.peerCredential)
      assertEquals(2U, joined.group.memberCount)
      assertEquals(1UL, engine.checkDirectGroup(alice.clientHandle, groupId, bob.signaturePublicKey, bob.credentialIdentity).epoch)
      val conversation = "50000000-0000-4000-8000-000000000001"
      val messageId = "60000000-0000-4000-8000-000000000001"
      val content = """{"v":1,"client_message_id":"$messageId","conversation_id":"$conversation","from_void_id":"110559210000000001","to_void_id":"110559210000000002","body":"Bound bridge 世界","created_at":1794000000123}""".toByteArray()
      val cipher = engine.encrypt(alice.clientHandle, groupId, content)
      val decrypted = engine.decryptDirectMessage(bob.clientHandle, groupId, cipher, alice.signaturePublicKey, alice.credentialIdentity,
        conversation, messageId, "110559210000000001", "110559210000000002")
      assertEquals("Bound bridge 世界", decrypted.body)
      assertEquals(1794000000123UL, decrypted.createdAt)
      val commit = engine.selfUpdate(alice.clientHandle, groupId)
      engine.mergePendingCommit(alice.clientHandle, groupId)
      assertEquals(2UL, engine.processDirectCommit(bob.clientHandle, groupId, commit, alice.signaturePublicKey, alice.credentialIdentity).epoch)
      val challenge = ("VOID-NOTHINGNESS:authenticate:v2\n" +
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8\n110559210000000001\n" +
        "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=\n2026-11-02T00:00:10.123456789Z\n" +
        "aHR0cHM6Ly9hcGkudm9pZGNoYXRzLnh5ei9kbTlwWkc1dmRHaHBibWR1WlhOeg==\n" +
        "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8\n").toByteArray()
      // Android validates live account/audience/expiry before this restricted Rust API.
      assertEquals(64, engine.signAuthChallenge(alice.clientHandle, challenge).size)
    } finally { engine.destroy() }
  }
}
