package com.grafium.app

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import android.util.AtomicFile
import com.k2fsa.sherpa.onnx.OfflineTts
import com.k2fsa.sherpa.onnx.OfflineTtsConfig
import com.k2fsa.sherpa.onnx.OfflineTtsModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsVitsModelConfig
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.Proxy
import java.net.URL
import java.security.MessageDigest
import java.util.UUID

/** A bundled sherpa-onnx CPU engine. There is deliberately no system TTS adapter. */
internal class ReaderSpeechEngine(
  private val context: Context,
  private val preflight: (File, JSONObject) -> Unit = { directory, manifest ->
    ReaderOfflineSpeech.validatePackage(context, directory, manifest)
  },
  private val engineFactory: (OfflineTtsConfig) -> OfflineTts = { config ->
    OfflineTts(assetManager = null, config = config)
  }
) {
  private val root = File(context.noBackupFilesDir, "private-reader/voices").apply { mkdirs() }
  private var engine: OfflineTts? = null
  private var loadedId: String? = null

  fun status(): JSONObject {
    val (voices, errors) = installed()
    val saved = try { selection() } catch (_: Exception) {
      errors.put(JSONObject().put("id", "saved-selection").put("available", false)
        .put("error", "VOICE_SELECTION_CORRUPT: select an installed voice again to repair the saved selection"))
      null
    }
    return JSONObject().put("available", true).put("runtime", "sherpa-vits-v1")
      .put("runtimeVersion", "1.13.8").put("selection", saved ?: JSONObject.NULL)
      .put("installed", voices).put("installationErrors", errors)
      .put("reason", if (errors.length() > 0)
        "An installed voice package or saved selection is corrupt or incomplete. Inspect installationErrors; original files were retained."
        else "An installed, compatible licensed voice is required. Physical-device synthesis is unverified.")
  }

  private fun selection(): JSONObject? = File(root, "selection.json").takeIf { it.exists() }?.let {
    require(it.isFile && it.length() <= 4096) { "VOICE_SELECTION_CORRUPT" }
    JSONObject(it.readText()).also { value ->
      require(validId(value.getString("voice_id")) && value.getString("language").length in 1..63) { "VOICE_SELECTION_CORRUPT" }
    }
  }
  fun selectedVoiceId(): String? = selection()?.getString("voice_id")
  private fun installed(): Pair<JSONArray, JSONArray> {
    val values = JSONArray()
    val errors = JSONArray()
    File(root, "installed").listFiles()?.filter { it.isDirectory }?.forEach { directory ->
      try {
        val manifestFile = File(directory, "manifest.json")
        require(manifestFile.isFile && manifestFile.length() <= 512 * 1024) { "VOICE_MANIFEST_MISSING_OR_OVERSIZED" }
        val manifest = validate(JSONObject(manifestFile.readText()))
        require(manifest.getString("id") == directory.name) { "VOICE_PACKAGE_ID_MISMATCH" }
        val artifacts = manifest.getJSONArray("artifacts")
        for (i in 0 until artifacts.length()) {
          val artifact = artifacts.getJSONObject(i)
          val file = File(directory, artifact.getString("path"))
          require(file.isFile && file.length() == artifact.getLong("bytes")) { "VOICE_FILES_MISSING_OR_CHANGED" }
        }
        values.put(manifest)
      } catch (_: Exception) {
        errors.put(JSONObject().put("id", directory.name.takeIf(::validId) ?: "unknown-package")
          .put("available", false).put("error", "VOICE_PACKAGE_CORRUPT: manifest or installed artifacts are invalid; original package files were retained"))
      }
    }
    return values to errors
  }
  fun select(id: String, language: String): JSONObject {
    require(validId(id)) { "INVALID_VOICE_ID" }
    val directory = File(root, "installed/$id")
    val manifest = validate(JSONObject(File(directory, "manifest.json").readText()))
    require(manifest.getString("language").equals(language, true)) { "VOICE_LANGUAGE_MISMATCH" }
    preflight(directory, manifest)
    engine?.release()
    engine = null
    loadedId = null
    val value = JSONObject().put("voice_id", id).put("language", language)
    atomic(File(root, "selection.json"), value.toString().toByteArray())
    return value
  }

  fun importTree(tree: Uri): JSONObject {
    ReaderPolicy.validateProvider(context, tree)
    fun find(parent: String, name: String): String {
      val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent)
      context.contentResolver.query(uri, arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use { cursor ->
        var count = 0
        while (cursor.moveToNext()) {
          require(++count <= 4096) { "VOICE_DIRECTORY_LIMIT" }
          if (cursor.getString(1) == name) return cursor.getString(0)
        }
      }
      throw IllegalArgumentException("VOICE_FILE_MISSING")
    }
    fun open(path: String): InputStream {
      require(safePath(path)) { "INVALID_VOICE_PATH" }
      var document = DocumentsContract.getTreeDocumentId(tree)
      path.split('/').forEach { document = find(document, it) }
      return context.contentResolver.openInputStream(DocumentsContract.buildDocumentUriUsingTree(tree, document))
        ?: throw IllegalStateException("VOICE_FILE_MISSING")
    }
    val manifest = open("manifest.json").use { input ->
      val bytes = input.readBytesBounded(512 * 1024)
      validate(JSONObject(bytes.toString(Charsets.UTF_8)))
    }
    return install(manifest, ::open)
  }

  fun download(manifest: JSONObject, authorized: Boolean): JSONObject {
    require(authorized) { "VOICE_DOWNLOAD_NEEDS_EXPLICIT_CONSENT" }
    validate(manifest)
    val artifacts = manifest.getJSONArray("artifacts")
    val urls = (0 until artifacts.length()).associate {
      val item = artifacts.getJSONObject(it)
      item.getString("path") to checkedUrl(item.getString("url"))
    }
    return install(manifest) { path ->
      val url = urls.getValue(path)
      require(java.net.InetAddress.getAllByName(url.host).all {
        !it.isAnyLocalAddress && !it.isLoopbackAddress && !it.isSiteLocalAddress && !it.isLinkLocalAddress
      }) { "VOICE_DOWNLOAD_REQUIRES_PUBLIC_HOST" }
      val connection = url.openConnection(Proxy.NO_PROXY) as HttpURLConnection
      connection.instanceFollowRedirects = false
      connection.connectTimeout = 20_000
      connection.readTimeout = 60_000
      connection.connect()
      require(connection.responseCode == 200) { "VOICE_DOWNLOAD_FAILED: redirects are not permitted" }
      object : java.io.FilterInputStream(connection.inputStream) {
        override fun close() { try { super.close() } finally { connection.disconnect() } }
      }
    }
  }

  internal fun install(manifest: JSONObject, source: (String) -> InputStream): JSONObject {
    validate(manifest)
    val id = manifest.getString("id")
    val destination = File(root, "installed/$id")
    require(!destination.exists()) { "VOICE_ALREADY_INSTALLED: use a distinct package ID for a replacement" }
    val staging = File(root, "staging-${UUID.randomUUID()}").apply { mkdirs() }
    try {
      val artifacts = manifest.getJSONArray("artifacts")
      for (i in 0 until artifacts.length()) {
        val item = artifacts.getJSONObject(i)
        val target = File(staging, item.getString("path"))
        target.parentFile!!.mkdirs()
        val digest = MessageDigest.getInstance("SHA-256")
        var total = 0L
        source(item.getString("path")).use { input ->
          target.outputStream().use { output ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
              val count = input.read(buffer)
              if (count < 0) break
              total += count
              require(total <= item.getLong("bytes")) { "VOICE_SIZE_MISMATCH" }
              digest.update(buffer, 0, count)
              output.write(buffer, 0, count)
            }
            output.fd.sync()
          }
        }
        require(total == item.getLong("bytes") && digest.digest().joinToString("") { "%02x".format(it) }
          .equals(item.getString("sha256"), true)) { "VOICE_INTEGRITY_MISMATCH" }
      }
      atomic(File(staging, "manifest.json"), manifest.toString().toByteArray())
      preflight(staging, manifest)
      destination.parentFile!!.mkdirs()
      require(staging.renameTo(destination)) { "VOICE_INSTALL_FAILED" }
      return manifest
    } finally {
      // Only this operation's generated staging files, never the user-selected source.
      if (staging.exists()) staging.deleteRecursively()
    }
  }

  fun synthesize(text: String, output: File, cancelled: () -> Boolean) {
    require(text.isNotBlank() && text.toByteArray().size <= 1500 && !text.contains('\u0000')) { "INVALID_SPEECH_SEGMENT" }
    val selected = selection() ?: throw IllegalStateException("OFFLINE_VOICE_REQUIRED")
    val id = selected.getString("voice_id")
    require(validId(id)) { "INVALID_VOICE_ID" }
    if (loadedId != id || engine == null) {
      engine?.release()
      engine = null
      loadedId = null
      val packageDir = File(root, "installed/$id")
      val manifest = validate(JSONObject(File(packageDir, "manifest.json").readText()))
      val artifacts = manifest.getJSONArray("artifacts")
      for (i in 0 until artifacts.length()) {
        val artifact = artifacts.getJSONObject(i)
        val file = File(packageDir, artifact.getString("path"))
        require(file.isFile && file.length() == artifact.getLong("bytes")) { "VOICE_FILES_MISSING_OR_CHANGED" }
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
          val buffer = ByteArray(64 * 1024)
          while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            digest.update(buffer, 0, count)
          }
        }
        require(digest.digest().joinToString("") { "%02x".format(it) }.equals(artifact.getString("sha256"), true)) {
          "VOICE_INTEGRITY_MISMATCH"
        }
      }
      fun role(name: String): String = (0 until artifacts.length()).map { artifacts.getJSONObject(it) }
        .firstOrNull { it.getString("role") == name }?.getString("path")
        ?.let { File(packageDir, it).absolutePath } ?: ""
      require(role("model").isNotEmpty() && role("tokens").isNotEmpty()) { "VOICE_FILES_MISSING" }
      preflight(packageDir, manifest)
      require(!cancelled()) { "SPEECH_CANCELLED" }
      engine = engineFactory(OfflineTtsConfig(model = OfflineTtsModelConfig(
        vits = OfflineTtsVitsModelConfig(model = role("model"), tokens = role("tokens"),
          lexicon = role("lexicon"), dataDir = File(packageDir, "espeak-ng-data").takeIf { it.isDirectory }?.absolutePath ?: ""),
        numThreads = 2, debug = false, provider = "cpu"), maxNumSentences = 1))
      loadedId = id
    }
    require(!cancelled()) { "SPEECH_CANCELLED" }
    val audio = engine!!.generateWithCallback(text, sid = 0, speed = 1f) { if (cancelled()) 0 else 1 }
    require(!cancelled()) { "SPEECH_CANCELLED" }
    require(audio.samples.size <= 4_000_000 && audio.sampleRate in 8000..48000) { "SPEECH_OUTPUT_LIMIT" }
    require(audio.save(output.absolutePath)) { "SPEECH_AUDIO_WRITE_FAILED" }
  }

  fun release() { engine?.release(); engine = null; loadedId = null }

  companion object {
    private fun validId(value: String) = Regex("[A-Za-z0-9_-]{1,96}").matches(value)
    private fun safePath(value: String) = value.length in 1..240 && !value.contains(Regex("[\\\\:\\x00]")) &&
      value.split('/').all { it.isNotEmpty() && it != "." && it != ".." }
    private fun checkedUrl(value: String): URL {
      val uri = java.net.URI(value)
      val host = uri.host.orEmpty()
      require(uri.scheme == "https" && uri.userInfo == null && uri.fragment == null && uri.query == null &&
        host.contains('.') && !host.endsWith(".local") && !host.endsWith(".localhost") &&
        !Regex("[0-9.]+").matches(host) && !host.contains(':')) { "INVALID_VOICE_DOWNLOAD_URL" }
      return uri.toURL()
    }
    internal fun validate(manifest: JSONObject): JSONObject {
      require(manifest.getInt("schema_version") == 1 && validId(manifest.getString("id"))) { "INVALID_VOICE_MANIFEST" }
      require(manifest.getString("runtime") == "sherpa-vits-v1") { "ANDROID_REQUIRES_SHERPA_VITS_VOICE" }
      require(manifest.getString("name").length in 1..256 &&
        Regex("[A-Za-z][A-Za-z0-9]{0,7}(-[A-Za-z0-9]{1,8})*").matches(manifest.getString("language")) &&
        manifest.getString("language").length <= 63 && manifest.getInt("sample_rate") in 8000..48000) { "INVALID_VOICE_METADATA" }
      require(manifest.getString("license").length in 1..256) { "VOICE_LICENSE_REQUIRED" }
      checkedUrl(manifest.getString("license_url"))
      val artifacts = manifest.getJSONArray("artifacts")
      require(artifacts.length() in 1..2048) { "VOICE_ARTIFACT_LIMIT" }
      val paths = HashSet<String>()
      var total = 0L
      val roles = HashMap<String, Int>()
      for (i in 0 until artifacts.length()) {
        val artifact = artifacts.getJSONObject(i)
        val path = artifact.getString("path")
        val role = artifact.getString("role")
        val size = artifact.getLong("bytes")
        require(safePath(path) && paths.add(path) && path != "manifest.json" &&
          role in setOf("model", "tokens", "lexicon", "espeak", "license", "config") &&
          size in 1..512L * 1024 * 1024 &&
          (role == "model" || size <= 16 * 1024 * 1024) &&
          Regex("[a-fA-F0-9]{64}").matches(artifact.getString("sha256"))) { "INVALID_VOICE_ARTIFACT" }
        if (role == "espeak") require(path.startsWith("espeak-ng-data/")) { "ESPEAK_DIRECTORY_REQUIRED" }
        roles[role] = (roles[role] ?: 0) + 1
        total += size
      }
      require(total <= 768L * 1024 * 1024 && roles["model"] == 1 && roles["tokens"] == 1 &&
        (roles["license"] ?: 0) > 0) { "INCOMPATIBLE_OR_UNLICENSED_VOICE_PACKAGE" }
      return manifest
    }
    private fun atomic(file: File, bytes: ByteArray) {
      val atomic = AtomicFile(file)
      val output = atomic.startWrite()
      try { output.write(bytes); atomic.finishWrite(output) }
      catch (error: Exception) { atomic.failWrite(output); throw error }
    }
    private fun InputStream.readBytesBounded(limit: Int): ByteArray {
      val out = java.io.ByteArrayOutputStream()
      val buffer = ByteArray(8192)
      while (true) {
        val count = read(buffer)
        if (count < 0) break
        require(out.size() + count <= limit) { "VOICE_MANIFEST_LIMIT" }
        out.write(buffer, 0, count)
      }
      return out.toByteArray()
    }
  }
}
