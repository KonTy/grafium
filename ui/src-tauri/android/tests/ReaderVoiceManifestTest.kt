package com.grafium.app

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.RuntimeEnvironment
import java.io.File

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], manifest = Config.NONE)
class ReaderVoiceManifestTest {
  private fun manifest(): JSONObject = JSONObject().put("schema_version", 1).put("id", "test-voice")
    .put("name", "Test voice").put("language", "zu-ZA").put("runtime", "sherpa-vits-v1")
    .put("sample_rate", 22050).put("license", "MIT").put("license_url", "https://example.org/license")
    .put("artifacts", JSONArray().put(artifact("model.onnx", "model"))
      .put(artifact("tokens.txt", "tokens")).put(artifact("LICENSE", "license")))
  private fun artifact(path: String, role: String): JSONObject = JSONObject()
    .put("path", path).put("role", role).put("bytes", 10).put("sha256", "a".repeat(64))

  @Test fun languageIsNotLimitedToAnEnglishShortlist() {
    assertEquals("zu-ZA", ReaderSpeechEngine.validate(manifest()).getString("language"))
    assertEquals("zh-Hant-TW", ReaderSpeechEngine.validate(manifest().put("language", "zh-Hant-TW")).getString("language"))
  }
  @Test fun rejectsTraversalDuplicateAndUnboundedVoiceArtifacts() {
    for (path in listOf("../model.onnx", "/model.onnx", "C:\\model.onnx", "a//b", "a/./b")) {
      val value = manifest()
      value.getJSONArray("artifacts").getJSONObject(0).put("path", path)
      assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(value) }
    }
    val duplicate = manifest()
    duplicate.getJSONArray("artifacts").put(artifact("tokens.txt", "tokens"))
    assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(duplicate) }
    val huge = manifest()
    huge.getJSONArray("artifacts").getJSONObject(0).put("bytes", 513L * 1024 * 1024)
    assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(huge) }
  }
  @Test fun requiresLicenseIntegrityAndMatchingOfflineRuntime() {
    assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(manifest().put("license", "")) }
    assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(manifest().put("runtime", "system-tts")) }
    assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(manifest().put("license_url", "http://localhost/license")) }
    val badHash = manifest()
    badHash.getJSONArray("artifacts").getJSONObject(0).put("sha256", "not-a-hash")
    assertThrows(IllegalArgumentException::class.java) { ReaderSpeechEngine.validate(badHash) }
  }
  @Test fun corruptInstalledPackageAndSelectionRemainVisibleAsErrors() {
    val context = RuntimeEnvironment.getApplication()
    val root = File(context.noBackupFilesDir, "private-reader/voices")
    val broken = File(root, "installed/broken-test/manifest.json")
    broken.parentFile!!.mkdirs()
    broken.writeText("{broken")
    File(root, "selection.json").writeText("{broken")
    val status = ReaderSpeechEngine(context).status()
    val errors = status.getJSONArray("installationErrors")
    assertTrue((0 until errors.length()).any { errors.getJSONObject(it).getString("id") == "broken-test" })
    assertTrue((0 until errors.length()).any { errors.getJSONObject(it).getString("id") == "saved-selection" })
    assertTrue(status.getString("reason").contains("corrupt"))
    assertTrue(broken.isFile)
  }
}
