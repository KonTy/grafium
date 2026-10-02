package com.grafium.app

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Base64
import android.webkit.WebView
import androidx.core.content.ContextCompat
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONArray
import org.json.JSONObject
import java.lang.ref.WeakReference
import java.util.concurrent.Executors

/** Every request checks the caller frame and origin before even parsing its body. */
internal class PrivateReaderBridge(
  private val activity: MainActivity,
  webView: WebView,
  private val chooseLocation: (String) -> Unit,
  private val exportState: (String) -> Unit,
  private val restoreState: (String) -> Unit
) {
  private val view = WeakReference(webView)
  private val worker = Executors.newSingleThreadExecutor()
  private val main = Handler(Looper.getMainLooper())
  private var controller: MediaController? = null
  private var voicePickerId: String? = null
  private val origins = mutableSetOf("https://tauri.localhost", "http://tauri.localhost", "tauri://localhost").apply {
    if (BuildConfig.DEBUG) { add("http://localhost:5173"); add("http://127.0.0.1:5173") }
  }

  init {
    live = WeakReference(this)
    if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) &&
      WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
      WebViewCompat.addWebMessageListener(webView, "_PrivateReaderNative", origins) { _, message, origin, isMainFrame, _ ->
        if (isMainFrame && origins.contains(origin.toString().trimEnd('/'))) accept(message.data ?: "")
      }
      WebViewCompat.addDocumentStartJavaScript(webView, """
        if (window === window.top) {
          Object.defineProperty(window, 'PrivateReaderBridge', {value: Object.freeze({
            request: function (json) {
              if (typeof json !== 'string') throw new TypeError('JSON string required');
              window._PrivateReaderNative.postMessage(json);
            }
          }), writable: false, configurable: false});
        }
      """.trimIndent(), origins)
    }
  }

  private fun accept(raw: String) {
    if (raw.length > 18_000_000) return
    var id = ""
    try {
      val request = JSONObject(raw)
      id = request.getString("id")
      require(id.length in 1..128) { "INVALID_REQUEST_ID" }
      val command = request.getString("command")
      require(command == "restore" || raw.length <= 2_000_000) { "READER_REQUEST_TOO_LARGE" }
      val args = request.optJSONObject("args") ?: JSONObject()
      if (command in setOf("play", "narrationStart") && args.has("playbackRate"))
        ReaderPolicy.playbackRate(args.get("playbackRate"))
      val requestId = id
      when (command) {
        "pickLocation" -> chooseLocation(id)
        "importVoice" -> {
          chooseLocation(id)
          voicePickerId = id
        }
        "exportState" -> exportState(id)
        "restoreState" -> restoreState(id)
        "accessibilitySettings" -> {
          activity.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
          respond(id, JSONObject().put("opened", true))
        }
        "capabilities" -> respond(id, capabilities(activity))
        "state" -> respond(id, PrivateReaderService.instance?.state() ?: idleState())
        "setPlaybackRate" -> {
          val rate = ReaderPolicy.playbackRate(args.opt("rate"))
          val service = PrivateReaderService.instance ?: throw IllegalStateException("NO_ACTIVE_BOOK")
          respond(id, service.setPlaybackRate(rate))
        }
        "narrationStart" -> worker.execute {
          try {
            val hash = ReaderNarrationUploads.get(activity).verifyForStart(args.getString("bookId"))
            main.post { withService(requestId) { it.startNarration(JSONObject(args.toString()).put("_verifiedSourceHash", hash)) } }
          } catch (error: Exception) { fail(requestId, error) }
        }
        "play", "pause", "resume", "stop", "seek", "bookmark" -> {
          if (command == "bookmark" && args.has("bookId") && PrivateReaderService.instance?.activeBookId() != args.getString("bookId")) {
            background(id) { ReaderLibrary.get(activity).bookmark(args.getString("bookId"), null, 0, args.optString("note")) }
          } else withService(id) { service ->
            when (command) {
              "play" -> service.play(args)
              "pause" -> service.pause()
              "resume" -> service.resume()
              "stop" -> service.stop()
              "seek" -> service.seek(args.getLong("offsetMs"))
              else -> service.bookmark(args.optString("note"))
            }
          }
        }
        "library", "rescan", "remove", "reorder", "position", "relink", "volumeSettings", "readEpub",
        "set_favorite", "record_activity", "add_link", "external_position", "external_bookmark",
        "voiceStatus", "selectVoice", "downloadVoice", "narrationPrepare", "updateBookmark", "deleteBookmark", "bookmarkVisual", "restore",
        "narrationBegin", "narrationAppend", "narrationCommit", "narrationCancel", "narrationAbort" -> {
          if (command in setOf("remove", "relink", "reorder") &&
            PrivateReaderService.instance?.activeBookId() == args.optString("bookId"))
            throw IllegalStateException("ACTIVE_BOOK: select another book before changing its registration")
          if (command == "selectVoice") PrivateReaderService.instance?.stop()
          background(requestId) {
            val library = ReaderLibrary.get(activity)
            when (command) {
              "library" -> library.library()
              "set_favorite" -> {
                require(args.get("favorite") is Boolean) { "INVALID_LIBRARY_FAVORITE" }
                library.setFavorite(args.getString("bookId"), args.getBoolean("favorite"))
              }
              "record_activity" -> library.recordActivity(args.getString("bookId"),
                if (args.has("progress")) args.getJSONObject("progress") else null)
              "add_link" -> library.addLink(args.getString("title"), args.getString("kind"), args.getString("url"))
              "external_position" -> library.saveExternalPosition(args.getString("bookId"), args.getJSONObject("position"))
              "external_bookmark" -> library.externalBookmark(args.getString("bookId"), args.getJSONObject("position"), args.optString("note"))
              "rescan" -> library.rescan()
              "remove" -> { library.remove(args.getString("bookId")); library.library() }
              "reorder" -> { library.reorder(args.getString("bookId"), args.getJSONArray("trackIds")); library.library() }
              "position" -> library.position(args.getString("bookId"), args.get("locator"), args.optLong("offsetMs", 0))
              "relink" -> { library.relink(args.getString("bookId"), args.getString("replacementBookId")); library.library() }
              "volumeSettings" -> library.setVolume(args)
              "updateBookmark" -> library.updateBookmark(args.getString("bookId"), args.getString("bookmarkId"), args.getString("note"))
              "deleteBookmark" -> library.deleteBookmark(args.optString("bookId"), args.optString("bookmarkId"))
              "bookmarkVisual" -> library.bookmarkVisual(args.getString("bookId"), args.getJSONObject("position"), args.optString("note"))
              "restore" -> library.restore(args.getString("data"))
              "voiceStatus" -> ReaderSpeechEngine(activity).status()
              "selectVoice" -> ReaderSpeechEngine(activity).select(args.getString("voiceId"), args.getString("language"))
              "downloadVoice" -> ReaderSpeechEngine(activity).download(args.getJSONObject("manifest"), args.optBoolean("authorized"))
              "narrationBegin" -> ReaderNarrationUploads.get(activity).begin(args.getString("bookId"))
              "narrationAppend" -> ReaderNarrationUploads.get(activity).append(args.getString("uploadId"), args.getJSONArray("segments"))
              "narrationCommit" -> ReaderNarrationUploads.get(activity).commit(args.getString("uploadId"))
              "narrationCancel", "narrationAbort" -> ReaderNarrationUploads.get(activity).cancel(args.getString("uploadId"))
              "narrationPrepare" -> throw IllegalStateException("CANONICAL_NARRATION_REQUIRED: use narrationBegin, narrationAppend and narrationCommit")
              else -> readEpub(library, args.getString("bookId"))
            }
          }
        }
        else -> throw IllegalArgumentException("UNKNOWN_READER_COMMAND")
      }
    } catch (error: Exception) { fail(id, error) }
  }

  private fun readEpub(library: ReaderLibrary, bookId: String): JSONObject {
    val bytes = epubBytes(library, bookId)
    val hash = java.security.MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    return JSONObject().put("base64", Base64.encodeToString(bytes, Base64.NO_WRAP))
      .put("mime", "application/epub+zip").put("sourceHash", hash)
  }

  private fun epubBytes(library: ReaderLibrary, bookId: String): ByteArray {
    val book = library.book(bookId)
    require(book.getString("kind") == "epub") { "EPUB_REQUIRED" }
    val id = book.getJSONArray("tracks").getJSONObject(0).getString("id")
    val uri = library.verifyResource(bookId, id)
    val bytes = activity.contentResolver.openInputStream(uri)?.use { input ->
      val output = java.io.ByteArrayOutputStream()
      val buffer = ByteArray(64 * 1024)
      while (true) {
        val count = input.read(buffer)
        if (count < 0) break
        require(output.size() + count <= 32 * 1024 * 1024) { "EPUB_TOO_LARGE: Android reader limit is 32 MiB" }
        output.write(buffer, 0, count)
      }
      output.toByteArray()
    } ?: throw IllegalStateException("SOURCE_MISSING")
    return bytes
  }

  private fun withService(id: String, action: (PrivateReaderService) -> JSONObject) {
    fun run() {
      try {
        val service = PrivateReaderService.instance ?: throw IllegalStateException("PLAYBACK_SERVICE_UNAVAILABLE")
        respond(id, action(service))
      } catch (error: Exception) { fail(id, error) }
    }
    if (controller != null && PrivateReaderService.instance != null) { run(); return }
    val future = MediaController.Builder(activity.applicationContext,
      SessionToken(activity, ComponentName(activity, PrivateReaderService::class.java))).buildAsync()
    future.addListener({
      try { controller = future.get(); run() } catch (error: Exception) { fail(id, error) }
    }, ContextCompat.getMainExecutor(activity))
  }

  fun selectedLocation(id: String, uri: Uri?) {
    val voice = voicePickerId == id
    if (voice) voicePickerId = null
    if (uri == null) { respond(id, JSONObject.NULL); return }
    background(id) {
      ReaderPolicy.validateProvider(activity, uri)
      try { activity.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
      catch (_: SecurityException) { throw IllegalStateException("READ_GRANT_MISSING: select the local folder again") }
      if (voice) ReaderSpeechEngine(activity).importTree(uri)
      else ReaderLibrary.get(activity).apply { selectTree(uri) }.rescan()
    }
  }

  fun selectedExport(id: String, uri: Uri?) {
    if (uri == null) { respond(id, JSONObject.NULL); return }
    background(id) {
      ReaderPolicy.validateBackupDocument(activity, uri)
      val json = ReaderLibrary.get(activity).exportState()
      val bytes = json.toByteArray()
      activity.contentResolver.openOutputStream(uri, "wt")?.use {
        it.write(bytes)
        it.flush()
        if (it is java.io.FileOutputStream) it.fd.sync()
      }
        ?: throw IllegalStateException("EXPORT_FAILED")
      val expected = java.security.MessageDigest.getInstance("SHA-256").digest(bytes)
      val actual = java.security.MessageDigest.getInstance("SHA-256")
      var count = 0L
      activity.contentResolver.openInputStream(uri)?.use { input ->
        val buffer = ByteArray(64 * 1024)
        while (true) {
          val read = input.read(buffer)
          if (read < 0) break
          count += read
          require(count <= bytes.size) { "BACKUP_VERIFICATION_FAILED" }
          actual.update(buffer, 0, read)
        }
      } ?: throw IllegalStateException("BACKUP_VERIFICATION_FAILED")
      require(count == bytes.size.toLong() && expected.contentEquals(actual.digest())) { "BACKUP_VERIFICATION_FAILED" }
      JSONObject().put("exported", true).put("bytes", count).put("scope", "private-library-history")
    }
  }

  fun selectedRestore(id: String, uri: Uri?) {
    if (uri == null) { respond(id, JSONObject.NULL); return }
    background(id) {
      ReaderPolicy.validateBackupDocument(activity, uri)
      val bytes = activity.contentResolver.openInputStream(uri)?.use { input ->
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(64 * 1024)
        while (true) {
          val count = input.read(buffer)
          if (count < 0) break
          require(output.size() + count <= 16 * 1024 * 1024) { "RESTORE_SIZE_LIMIT: backup exceeds 16 MiB; existing state was not changed" }
          output.write(buffer, 0, count)
        }
        output.toByteArray()
      } ?: throw IllegalStateException("BACKUP_UNREADABLE: existing state was not changed")
      val json = try {
        Charsets.UTF_8.newDecoder().onMalformedInput(java.nio.charset.CodingErrorAction.REPORT)
          .onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT)
          .decode(java.nio.ByteBuffer.wrap(bytes)).toString()
      } catch (_: Exception) { throw IllegalArgumentException("INVALID_BACKUP_ENCODING: existing state was not changed") }
      try {
        val restored = ReaderLibrary.get(activity).restore(json)
        JSONObject().put("restored", true).put("restoreMode", restored.getString("restoreMode"))
          .put("restoreNotice", restored.getString("restoreNotice"))
      }
      catch (_: org.json.JSONException) {
        throw IllegalArgumentException("INVALID_BACKUP: select a Grafium Android private-reader backup; existing state was not changed")
      }
    }
  }

  private fun background(id: String, action: () -> Any) {
    worker.execute {
      try { respond(id, action()) }
      catch (error: Exception) { fail(id, error) }
      catch (_: LinkageError) { fail(id, IllegalStateException("OFFLINE_NATIVE_RUNTIME_UNAVAILABLE")) }
    }
  }
  private fun respond(id: String, result: Any) = dispatch("private-reader-response",
    JSONObject().put("id", id).put("ok", true).put("result", result))
  private fun fail(id: String, error: Exception) {
    val text = error.message.orEmpty()
    // Provider exceptions can include private absolute paths/URIs.
    val safe = if (Regex("^[A-Z_]+(?=:|$)").containsMatchIn(text)) text else "PRIVATE_READER_OPERATION_FAILED"
    dispatch("private-reader-response", JSONObject().put("id", id).put("ok", false).put("error", safe))
  }
  private fun dispatch(event: String, detail: JSONObject) {
    val json = JSONObject.quote(detail.toString()).replace("\u2028", "\\u2028").replace("\u2029", "\\u2029")
    main.post {
      val webView = view.get() ?: return@post
      val uri = webView.url?.let(Uri::parse) ?: return@post
      val origin = "${uri.scheme}://${uri.encodedAuthority}"
      if (!origins.contains(origin)) return@post
      webView.evaluateJavascript("if(window===window.top)window.dispatchEvent(new CustomEvent('$event',{detail:JSON.parse($json)}));", null)
    }
  }

  fun destroy() {
    controller?.release()
    controller = null
    worker.shutdown()
    if (live?.get() === this) live = null
  }
  companion object {
    private var live: WeakReference<PrivateReaderBridge>? = null
    fun publish(state: JSONObject) { live?.get()?.dispatch("private-reader-state", state) }
    fun publishCapabilities(context: Context) { live?.get()?.dispatch("private-reader-capabilities", capabilities(context)) }
    private fun idleState() = JSONObject().put("bookId", JSONObject.NULL).put("trackId", JSONObject.NULL)
      .put("offsetMs", 0).put("durationMs", 0).put("playing", false).put("buffering", false)
      .put("error", JSONObject.NULL).put("checkpointIntervalMs", 3000)
    fun capabilities(context: Context): JSONObject = JSONObject()
      .put("nativePlayback", true).put("localProviderOnly", true).put("checkpointIntervalMs", 3000)
      .put("volume", JSONObject().put("settings", ReaderLibrary.get(context).volume())
        .put("accessibilityConnected", ReaderVolumeService.connected)
        .put("gesture", "longPress").put("holdMs", 700)
        .put("lockedScreen", "unverified").put("screenOff", "unverified")
        .put("verification", "Physical Samsung, Pixel and Vivo devices have not been tested. OS/OEM key delivery may prevent locked or screen-off bookmarking.")
        .put("diagnostics", JSONObject(ReaderVolumeService.diagnostics.toString())))
      .put("cloudBackup", "Private reader state uses Android noBackupFilesDir; source media and user-selected exports follow their storage policy.")
      .put("backup", JSONObject().put("maxBytes", 16 * 1024 * 1024)
        .put("scope", "Library registration, progress and bookmarks; original media, voice models and generated speech are not included. Restore never grants source access or enables accessibility."))
      .put("offlineTts", true)
  }
}
