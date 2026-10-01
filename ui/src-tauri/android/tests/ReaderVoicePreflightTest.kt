package com.grafium.app

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File
import java.security.MessageDigest
import java.util.UUID

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], manifest = Config.NONE)
class ReaderVoicePreflightTest {
  private val context get() = RuntimeEnvironment.getApplication()
  private val root get() = File(context.noBackupFilesDir, "private-reader/voices").apply { mkdirs() }
  private val metadataError = "Sherpa model sample_rate does not match manifest; raw Piper ONNX is incompatible"

  private fun fixture(directory: File, id: String): JSONObject {
    directory.mkdirs()
    val artifacts = JSONArray()
    for ((path, role) in listOf("model.onnx" to "model", "tokens.txt" to "tokens", "LICENSE" to "license")) {
      val bytes = "synthetic test artifact: $path".toByteArray()
      File(directory, path).writeBytes(bytes)
      artifacts.put(JSONObject().put("path", path).put("role", role).put("bytes", bytes.size)
        .put("sha256", MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }))
    }
    return JSONObject().put("schema_version", 1).put("id", id).put("name", "Preflight test")
      .put("language", "en-US").put("runtime", "sherpa-vits-v1").put("sample_rate", 22050)
      .put("license", "MIT").put("license_url", "https://example.org/license").put("artifacts", artifacts)
      .also { File(directory, "manifest.json").writeText(it.toString()) }
  }

  private fun installed(): Pair<File, JSONObject> {
    val id = "preflight-${UUID.randomUUID()}"
    val directory = File(root, "installed/$id")
    val manifest = fixture(directory, id)
    File(root, "selection.json").writeText(JSONObject().put("voice_id", id).put("language", "en-US").toString())
    return directory to manifest
  }

  private fun rejectMetadata(): Unit {
    ReaderOfflineSpeech.validationResult(JSONObject().put("error", metadataError).toString())
    fail("Native metadata rejection must propagate")
  }

  @Test fun failedSharedPreflightNeverPublishesCopiedPackageOrChangesSource() {
    val id = "preflight-${UUID.randomUUID()}"
    val source = File(context.cacheDir, id)
    val manifest = fixture(source, id)
    val original = File(source, "model.onnx").readBytes()
    var calls = 0
    var staging: File? = null
    val engine = ReaderSpeechEngine(context, preflight = { directory, checked ->
      calls++
      staging = directory
      assertEquals(id, checked.getString("id"))
      assertTrue(directory.name.startsWith("staging-"))
      assertTrue(File(directory, "manifest.json").isFile)
      assertArrayEquals(original, File(directory, "model.onnx").readBytes())
      rejectMetadata()
    })

    val error = assertThrows(IllegalArgumentException::class.java) {
      engine.install(manifest) { File(source, it).inputStream() }
    }
    assertEquals("VOICE_PACKAGE_INVALID: $metadataError", error.message)
    assertEquals(1, calls)
    assertFalse(File(root, "installed/$id").exists())
    assertFalse(staging!!.exists())
    assertArrayEquals(original, File(source, "model.onnx").readBytes())
  }

  @Test fun failedSharedPreflightNeverConstructsEngineAndRetryRevalidates() {
    val (directory, manifest) = installed()
    var validations = 0
    var constructions = 0
    val engine = ReaderSpeechEngine(context, preflight = { checkedDirectory, checkedManifest ->
      validations++
      assertEquals(directory.canonicalFile, checkedDirectory.canonicalFile)
      assertEquals(manifest.getString("id"), checkedManifest.getString("id"))
      rejectMetadata()
    }, engineFactory = {
      constructions++
      throw AssertionError("Native engine must not be constructed after rejected preflight")
    })
    val output = File(context.cacheDir, "rejected-${UUID.randomUUID()}.wav")

    repeat(2) {
      val error = assertThrows(IllegalArgumentException::class.java) { engine.synthesize("Test", output) { false } }
      assertEquals("VOICE_PACKAGE_INVALID: $metadataError", error.message)
    }
    assertEquals(2, validations)
    assertEquals(0, constructions)
    assertFalse(output.exists())
    assertTrue(directory.isDirectory)
  }

  @Test fun constructorReceivesOnlyTheExactPreflightedPackage() {
    val (directory, _) = installed()
    val operations = mutableListOf<String>()
    val sentinel = IllegalStateException("TEST_STOP_BEFORE_NATIVE_CONSTRUCTOR")
    val engine = ReaderSpeechEngine(context, preflight = { checkedDirectory, _ ->
      assertEquals(directory.canonicalFile, checkedDirectory.canonicalFile)
      operations.add("preflight")
    }, engineFactory = { config ->
      operations.add("construct")
      assertEquals(File(directory, "model.onnx").absolutePath, config.model.vits.model)
      assertEquals("cpu", config.model.provider)
      throw sentinel
    })
    val error = assertThrows(IllegalStateException::class.java) {
      engine.synthesize("Test", File(context.cacheDir, "unused.wav")) { false }
    }
    assertSame(sentinel, error)
    assertEquals(listOf("preflight", "construct"), operations)
  }

  @Test fun rejectedSelectionPreservesPreviouslySavedVoice() {
    val (_, manifest) = installed()
    val selection = File(root, "selection.json")
    selection.writeText(JSONObject().put("voice_id", "previous").put("language", "en-US").toString())
    val original = selection.readBytes()
    val engine = ReaderSpeechEngine(context, preflight = { _, _ -> rejectMetadata() })

    assertThrows(IllegalArgumentException::class.java) { engine.select(manifest.getString("id"), "en-US") }
    assertArrayEquals(original, selection.readBytes())
  }

  @Test fun missingNativeValidatorFailsClosedWithoutConstructingEngine() {
    installed()
    var constructions = 0
    val engine = ReaderSpeechEngine(context, engineFactory = {
      constructions++
      throw AssertionError("A missing JNI validator must not permit engine construction")
    })
    val error = assertThrows(IllegalStateException::class.java) {
      engine.synthesize("Test", File(context.cacheDir, "unused.wav")) { false }
    }
    assertTrue(error.message!!.startsWith("VOICE_VALIDATION_UNAVAILABLE"))
    assertTrue(error.cause is LinkageError)
    assertEquals(0, constructions)
  }
}
