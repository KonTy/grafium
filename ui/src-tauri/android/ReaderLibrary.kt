package com.grafium.app

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import java.util.UUID

/** Only the platform external-storage provider is permitted: no cloud-provider fallback. */
internal object ReaderPolicy {
  const val LOCAL_AUTHORITY = "com.android.externalstorage.documents"
  /** Pages that may reach the native bridge. Android's WebView accepts only
   *  http(s) origins here and throws on anything else; Tauri serves Android
   *  pages from http(s)://tauri.localhost, never the desktop tauri://localhost. */
  fun bridgeOrigins(debug: Boolean): Set<String> {
    val origins = linkedSetOf("https://tauri.localhost", "http://tauri.localhost")
    if (debug) origins += listOf("http://localhost:5173", "http://127.0.0.1:5173")
    return origins
  }
  fun playbackRate(value: Any?): Float {
    require(value is Number && value.toDouble().isFinite() && value.toDouble() in 0.5..4.0) {
      "INVALID_PLAYBACK_RATE: expected a finite number from 0.5 to 4"
    }
    return value.toFloat()
  }
  val audioExtensions = setOf("mp3", "m4a", "m4b", "aac", "ogg", "opus", "flac", "wav")
  fun normalizeLink(kind: String, value: String): String {
    require(kind in setOf("audio", "video", "youtube") && value.length <= 8192 &&
      value.none { it.isISOControl() || it.isWhitespace() } && !value.contains('\\')) { "INVALID_LIBRARY_LINK" }
    val url = try { java.net.URI(value) } catch (_: Exception) { throw IllegalArgumentException("INVALID_LIBRARY_URL") }
    require(url.scheme?.lowercase() in setOf("http", "https") && !url.host.isNullOrEmpty() &&
      url.rawUserInfo == null && !url.rawAuthority.contains('@')) { "HTTP_LINK_WITHOUT_CREDENTIALS_REQUIRED" }
    if (kind != "youtube") {
      val scheme = url.scheme.lowercase()
      val port = if ((scheme == "https" && url.port == 443) || (scheme == "http" && url.port == 80)) -1 else url.port
      return java.net.URI(scheme, null, url.host.lowercase(), port, null, null, null).toString() +
        (url.rawPath.ifEmpty { "/" }) + (url.rawQuery?.let { "?$it" } ?: "") + (url.rawFragment?.let { "#$it" } ?: "")
    }
    require(url.port == -1 || url.port == if (url.scheme.lowercase() == "https") 443 else 80) { "INVALID_YOUTUBE_PORT" }
    val host = url.host.lowercase()
    val parts = url.path.removePrefix("/").split("/")
    val id = if (host in setOf("youtu.be", "www.youtu.be") && parts.size == 1) parts[0]
      else if (host in setOf("youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com",
        "youtube-nocookie.com", "www.youtube-nocookie.com")) {
        if (url.path == "/watch") Uri.parse(value).getQueryParameter("v")
        else if (parts.size == 2 && parts[0] in setOf("embed", "shorts", "live")) parts[1] else null
      } else null
    require(id != null && Regex("[A-Za-z0-9_-]{11}").matches(id)) { "KNOWN_YOUTUBE_VIDEO_REQUIRED" }
    return "https://www.youtube.com/watch?v=$id"
  }

  fun progress(value: JSONObject): JSONObject {
    require(value.keys().asSequence().toSet() == setOf("position", "total", "anchor", "label")) { "INVALID_LIBRARY_PROGRESS" }
    val position = value.get("position")
    val total = value.get("total")
    val anchor = value.get("anchor")
    val label = value.get("label")
    require(position is Number && total is Number && anchor is String && label is String) { "INVALID_LIBRARY_PROGRESS" }
    require(position.toDouble().isFinite() && total.toDouble().isFinite() &&
      position.toDouble() in 0.0..31_536_000_000.0 && total.toDouble() in 0.0..31_536_000_000.0 &&
      (total.toDouble() == 0.0 || position.toDouble() <= total.toDouble()) &&
      anchor.length <= 8192 && label.length <= 1024 && (anchor + label).none { it.isISOControl() }) { "INVALID_LIBRARY_PROGRESS" }
    return JSONObject(value.toString())
  }

  fun libraryMetadata(book: JSONObject) {
    if (!book.has("favorite")) book.put("favorite", false)
    if (!book.has("lastUsedAt")) book.put("lastUsedAt", 0)
    require(book.get("favorite") is Boolean) { "INVALID_LIBRARY_FAVORITE" }
    val used = book.get("lastUsedAt")
    require(used is Number && used.toDouble() == used.toLong().toDouble() &&
      used.toLong() in 0..9_007_199_254_740_991L) { "INVALID_LIBRARY_ACTIVITY" }
    if (book.has("progress")) progress(book.getJSONObject("progress"))
    if (book.has("sourceUrl")) {
      require(book.get("sourceUrl") is String && normalizeLink(book.getString("kind"), book.getString("sourceUrl")) ==
        book.getString("sourceUrl") && book.getJSONArray("tracks").length() == 0 &&
        listOf("tree", "documentId", "bindingId").none { book.has(it) }) { "INVALID_EXTERNAL_LIBRARY_ITEM" }
      mediaPosition(book.getJSONObject("position"))
      val marks = book.getJSONArray("bookmarks")
      for (i in 0 until marks.length()) mediaPosition(marks.getJSONObject(i).getJSONObject("position"))
    } else require(book.getString("kind") in setOf("audio", "epub")) { "UNSUPPORTED_LOCAL_LIBRARY_KIND" }
  }

  fun libraryVersion(document: JSONObject) {
    if (!document.has("libraryVersion")) return
    val version = document.get("libraryVersion")
    require(version is Number && version.toDouble() == version.toInt().toDouble() &&
      version.toInt() in 1..2) { "UNSUPPORTED_LIBRARY_VERSION" }
  }

  fun mediaPosition(raw: JSONObject): JSONObject {
    require(raw.keys().asSequence().toSet() == setOf("offsetMs")) { "EXTERNAL_POSITION_REQUIRED" }
    val offset = raw.get("offsetMs")
    require(offset is Number && offset.toDouble() == offset.toLong().toDouble() &&
      offset.toLong() in 0..31_536_000_000L) { "INVALID_POSITION" }
    return JSONObject().put("offsetMs", offset.toLong())
  }
  fun stableId(value: String): String = MessageDigest.getInstance("SHA-256")
    .digest(value.toByteArray()).joinToString("") { "%02x".format(it) }
  fun naturalKey(value: String): String =
    Regex("\\d+").replace(value.lowercase()) { it.value.trimStart('0').padStart(24, '0') }
  fun validateTree(uri: Uri) {
    require(uri.scheme == "content" && uri.authority == LOCAL_AUTHORITY &&
      DocumentsContract.isTreeUri(uri)) { "LOCAL_SOURCE_REQUIRED: choose internal storage or an SD card" }
  }
  fun validateProvider(context: Context, uri: Uri) {
    validateTree(uri)
    val provider = context.packageManager.resolveContentProvider(LOCAL_AUTHORITY, 0)
    require(provider?.packageName == "com.android.externalstorage" &&
      (provider.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_SYSTEM) != 0) {
      "LOCAL_SOURCE_REQUIRED: only the system local-storage provider is supported"
    }
  }
  fun validateBackupDocument(context: Context, uri: Uri) {
    val packages = mapOf(LOCAL_AUTHORITY to "com.android.externalstorage",
      "com.android.providers.downloads.documents" to "com.android.providers.downloads")
    val expected = packages[uri.authority]
    val provider = uri.authority?.let { context.packageManager.resolveContentProvider(it, 0) }
    require(uri.scheme == "content" && expected != null && provider?.packageName == expected &&
      (provider.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_SYSTEM) != 0) {
      "LOCAL_BACKUP_REQUIRED: choose internal storage, an SD card, or local Downloads"
    }
  }
}

internal class ReaderLibrary private constructor(private val context: Context) {
  private val file = AtomicFile(File(context.noBackupFilesDir, "private-reader/library.json"))
  private var data: JSONObject

  init {
    file.baseFile.parentFile!!.mkdirs()
    data = if (file.baseFile.exists() || File(file.baseFile.path + ".bak").exists()) {
      try { JSONObject(file.openRead().bufferedReader().use { it.readText() }) }
      catch (_: Exception) { throw IllegalStateException("PRIVATE_STATE_UNREADABLE: preserve app data and restore a local backup") }
    } else JSONObject().put("books", JSONArray()).put("volume", JSONObject()
      .put("enabled", false).put("key", "up").put("gesture", "longPress"))
    ReaderPolicy.libraryVersion(data)
    val books = data.getJSONArray("books")
    for (i in 0 until books.length()) ReaderPolicy.libraryMetadata(books.getJSONObject(i))
  }

  private var committed: String = data.toString()

  @Synchronized private fun save() {
    var stream: java.io.FileOutputStream? = null
    try {
      data.put("libraryVersion", 2)
      stream = file.startWrite()
      stream.write(data.toString().toByteArray())
      file.finishWrite(stream)
      committed = data.toString()
    } catch (error: Exception) {
      if (stream != null) file.failWrite(stream)
      data = JSONObject(committed)
      throw IllegalStateException("PRIVATE_STATE_WRITE_FAILED", error)
    }
  }

  @Synchronized fun books(): JSONArray {
    val result = JSONArray()
    val books = data.getJSONArray("books")
    for (i in 0 until books.length()) {
      val book = JSONObject(books.getJSONObject(i).toString())
      book.remove("documentId")
      book.remove("tree")
      book.remove("bindingId")
      val tracks = book.getJSONArray("tracks")
      for (j in 0 until tracks.length()) {
        tracks.getJSONObject(j).put("available", book.optBoolean("available"))
        tracks.getJSONObject(j).remove("documentId")
        tracks.getJSONObject(j).remove("size")
        tracks.getJSONObject(j).remove("modified")
      }
      val bookmarks = book.getJSONArray("bookmarks")
      for (j in 0 until bookmarks.length()) bookmarks.getJSONObject(j).put("bookId", book.getString("id"))
      result.put(book)
    }
    return result
  }

  @Synchronized fun library(): JSONObject = JSONObject().put("books", books())
    .put("configured", data.has("tree")).put("locationLabel", data.optString("locationLabel", ""))
    .put("error", data.optString("error", ""))

  @Synchronized fun setFavorite(bookId: String, favorite: Boolean): JSONObject {
    mutableBook(bookId).put("favorite", favorite)
    save()
    return library()
  }

  @Synchronized fun recordActivity(bookId: String, progress: JSONObject?): JSONObject {
    val checked = progress?.let { ReaderPolicy.progress(it) }
    val book = mutableBook(bookId)
    book.put("lastUsedAt", System.currentTimeMillis())
    if (checked != null) book.put("progress", checked)
    save()
    return library()
  }

  @Synchronized fun addLink(title: String, kind: String, url: String): JSONObject {
    val normalized = ReaderPolicy.normalizeLink(kind, url)
    val trimmed = title.trim()
    require(trimmed.length in 1..1024 && trimmed.none { it.isISOControl() }) { "INVALID_LIBRARY_TITLE" }
    val books = data.getJSONArray("books")
    for (i in 0 until books.length()) {
      val book = books.getJSONObject(i)
      if (book.getString("kind") == kind && book.optString("sourceUrl") == normalized) return library()
    }
    require(books.length() < 10000) { "LIBRARY_LIMIT" }
    books.put(JSONObject().put("id", ReaderPolicy.stableId(UUID.randomUUID().toString()))
      .put("title", trimmed).put("kind", kind).put("sourceUrl", normalized).put("available", true)
      .put("tracks", JSONArray()).put("bookmarks", JSONArray()).put("position", JSONObject().put("offsetMs", 0))
      .put("favorite", false).put("lastUsedAt", 0))
    save()
    return library()
  }

  private fun externalPosition(book: JSONObject, raw: JSONObject): JSONObject {
    require(book.has("sourceUrl")) { "EXTERNAL_POSITION_REQUIRED" }
    return ReaderPolicy.mediaPosition(raw)
  }

  @Synchronized fun saveExternalPosition(bookId: String, raw: JSONObject): JSONObject {
    val book = mutableBook(bookId)
    val position = externalPosition(book, raw)
    book.put("position", position)
    save()
    return JSONObject(position.toString())
  }

  @Synchronized fun externalBookmark(bookId: String, raw: JSONObject, note: String): JSONObject =
    bookmarkAt(bookId, externalPosition(mutableBook(bookId), raw), note)
  @Synchronized fun volume(): JSONObject = JSONObject(data.getJSONObject("volume").toString())
  @Synchronized fun playbackRate(mode: String): Float {
    require(mode in setOf("audio", "tts")) { "INVALID_PLAYBACK_MODE" }
    if (!data.has("playbackRates")) return 1f
    val rates = data.getJSONObject("playbackRates")
    return if (rates.has(mode)) ReaderPolicy.playbackRate(rates.get(mode)) else 1f
  }
  @Synchronized fun setPlaybackRate(mode: String, value: Any?) {
    require(mode in setOf("audio", "tts")) { "INVALID_PLAYBACK_MODE" }
    val rate = ReaderPolicy.playbackRate(value)
    val rates = if (data.has("playbackRates")) JSONObject(data.getJSONObject("playbackRates").toString()) else JSONObject()
    data.put("playbackRates", rates.put(mode, rate))
    save()
  }
  @Synchronized fun setVolume(args: JSONObject): JSONObject {
    require(args.optString("key", "up") in setOf("up", "down")) { "INVALID_VOLUME_KEY" }
    require(args.optString("gesture", "longPress") == "longPress") { "ONLY_LONG_PRESS_SUPPORTED" }
    data.put("volume", JSONObject().put("enabled", args.optBoolean("enabled"))
      .put("key", args.optString("key", "up")).put("gesture", "longPress"))
    save()
    return volume()
  }

  @Synchronized fun book(id: String): JSONObject {
    val books = data.getJSONArray("books")
    for (i in 0 until books.length()) if (books.getJSONObject(i).getString("id") == id)
      return JSONObject(books.getJSONObject(i).toString())
    throw IllegalArgumentException("BOOK_NOT_REGISTERED")
  }

  private fun mutableBook(id: String): JSONObject {
    val books = data.getJSONArray("books")
    for (i in 0 until books.length()) if (books.getJSONObject(i).getString("id") == id)
      return books.getJSONObject(i)
    throw IllegalArgumentException("BOOK_NOT_REGISTERED")
  }

  @Synchronized fun selectTree(uri: Uri) {
    ReaderPolicy.validateProvider(context, uri)
    require(context.contentResolver.persistedUriPermissions.any { it.uri == uri && it.isReadPermission }) {
      "READ_GRANT_MISSING: select the library again"
    }
    data.put("tree", uri.toString())
    data.put("locationLabel", DocumentsContract.getTreeDocumentId(uri).substringAfter(':').ifEmpty { "Local storage" })
    data.remove("error")
    save()
  }

  private data class Document(val id: String, val name: String, val mime: String, val size: Long, val modified: Long)
  private fun children(tree: Uri, parent: String): List<Document> {
    val childUri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent)
    val columns = arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID,
      DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_MIME_TYPE,
      DocumentsContract.Document.COLUMN_SIZE, DocumentsContract.Document.COLUMN_LAST_MODIFIED)
    return context.contentResolver.query(childUri, columns, null, null, null)?.use { cursor ->
      val result = ArrayList<Document>()
      while (cursor.moveToNext()) {
        require(result.size < 10000) { "LIBRARY_LIMIT: more than 10000 documents in one folder" }
        result.add(Document(cursor.getString(0), cursor.getString(1), cursor.getString(2),
          cursor.getLong(3), cursor.getLong(4)))
      }
      result.sortedBy { ReaderPolicy.naturalKey(it.name) }
    } ?: throw IllegalStateException("SOURCE_MISSING: provider returned no directory")
  }

  /** Build outside the lock; checkpoints must not wait on provider traversal. */
  fun rescan(): JSONObject {
    val treeString = synchronized(this) { data.optString("tree") }
    require(treeString.isNotEmpty()) { "LOCATION_REQUIRED: select a local library" }
    val tree = Uri.parse(treeString)
    ReaderPolicy.validateProvider(context, tree)
    try {
      require(context.contentResolver.persistedUriPermissions.any { it.uri == tree && it.isReadPermission }) {
        "READ_GRANT_MISSING: relink the library location"
      }
      val found = LinkedHashMap<String, JSONObject>()
      var count = 0
      val visited = HashSet<String>()
      fun walk(parent: String, path: String, audioRoot: Pair<String, String>?, depth: Int) {
        require(depth <= 32) { "LIBRARY_LIMIT: directory nesting exceeds 32" }
        require(visited.add(parent)) { "INVALID_SOURCE: provider directory cycle" }
        for (doc in children(tree, parent)) {
          require(++count <= 10000) { "LIBRARY_LIMIT: more than 10000 documents" }
          val relative = if (path.isEmpty()) doc.name else "$path/${doc.name}"
          if (doc.mime == DocumentsContract.Document.MIME_TYPE_DIR) {
            val disc = Regex("(?i)(disc|disk|cd)[ ._-]*\\d+").matches(doc.name)
            walk(doc.id, relative, if (disc && audioRoot != null) audioRoot else (doc.id to doc.name), depth + 1)
            continue
          }
          val extension = doc.name.substringAfterLast('.', "").lowercase()
          val kind = if (extension == "epub") "epub" else if (extension in ReaderPolicy.audioExtensions) "audio" else continue
          val rootId = if (kind == "audio") audioRoot?.first ?: doc.id else doc.id
          val title = if (kind == "audio") audioRoot?.second ?: doc.name.substringBeforeLast('.') else doc.name.substringBeforeLast('.')
          val id = ReaderPolicy.stableId("$treeString|$kind|$rootId")
          val book = found.getOrPut(id) { JSONObject().put("id", id).put("title", title).put("kind", kind)
            .put("available", true).put("tree", treeString).put("documentId", doc.id)
            .put("tracks", JSONArray()).put("position", JSONObject().put("offsetMs", 0))
            .put("bookmarks", JSONArray()) }
          val trackId = ReaderPolicy.stableId("$treeString|${doc.id}|${doc.size}|${doc.modified}")
          book.getJSONArray("tracks").put(JSONObject().put("id", trackId).put("title", doc.name)
            .put("relativePath", relative).put("documentId", doc.id).put("size", doc.size).put("modified", doc.modified))
        }
      }
      walk(DocumentsContract.getTreeDocumentId(tree), "", null, 0)
      synchronized(this) {
        require(data.optString("tree") == treeString) { "LOCATION_CHANGED: scan again" }
        val old = data.getJSONArray("books")
        val merged = JSONArray()
        for (i in 0 until old.length()) {
          val previous = old.getJSONObject(i)
          if (previous.has("sourceUrl")) { merged.put(previous); continue }
          val bindingId = previous.optString("bindingId", previous.getString("id"))
          val fresh = found.remove(bindingId)
          if (fresh == null) {
            previous.put("available", false).put("error", "SOURCE_MISSING: rescan or explicitly relink")
            merged.put(previous)
          } else {
            fresh.put("id", previous.getString("id")).put("bindingId", bindingId)
            fresh.put("position", previous.getJSONObject("position")).put("bookmarks", previous.getJSONArray("bookmarks"))
            for (field in listOf("favorite", "lastUsedAt", "progress")) if (previous.has(field)) fresh.put(field, previous.get(field))
            if (previous.has("order")) applyOrder(fresh, previous.getJSONArray("order"))
            val trackId = fresh.getJSONObject("position").optString("trackId")
            if (trackId.isNotEmpty() && !hasTrack(fresh, trackId))
              fresh.put("error", "SAVED_TRACK_MISSING_OR_REPLACED: choose a track explicitly; history is retained")
            merged.put(fresh)
          }
        }
        val excluded = data.optJSONArray("excluded") ?: JSONArray()
        val excludedIds = (0 until excluded.length()).map { excluded.getString(it) }.toSet()
        found.values.filter { it.getString("id") !in excludedIds }.forEach { ReaderPolicy.libraryMetadata(it); merged.put(it) }
        data.put("books", merged).remove("error")
        save()
        return library()
      }
    } catch (error: Exception) {
      synchronized(this) {
        val code = error.message?.substringBefore(":").orEmpty()
        data.put("error", if (Regex("[A-Z_]+").matches(code)) code
          else if (error is SecurityException) "READ_GRANT_MISSING" else "SOURCE_UNAVAILABLE")
        val books = data.getJSONArray("books")
        for (i in 0 until books.length()) {
          val book = books.getJSONObject(i)
          if (!book.has("sourceUrl")) book.put("available", false)
        }
        save()
      }
      throw error
    }
  }

  private fun hasTrack(book: JSONObject, id: String): Boolean {
    val tracks = book.getJSONArray("tracks")
    return (0 until tracks.length()).any { tracks.getJSONObject(it).getString("id") == id }
  }

  @Synchronized fun resource(bookId: String, trackId: String): Uri {
    val book = mutableBook(bookId)
    require(!book.has("sourceUrl")) { "EXTERNAL_MEDIA_NOT_A_SAF_RESOURCE" }
    require(book.optBoolean("available")) { "SOURCE_MISSING: rescan or relink this book" }
    val tree = Uri.parse(book.getString("tree"))
    ReaderPolicy.validateProvider(context, tree)
    require(context.contentResolver.persistedUriPermissions.any { it.uri == tree && it.isReadPermission }) {
      "READ_GRANT_MISSING: relink the library location"
    }
    val tracks = book.getJSONArray("tracks")
    val track = (0 until tracks.length()).map { tracks.getJSONObject(it) }.find { it.getString("id") == trackId }
      ?: throw IllegalArgumentException("TRACK_MISSING_OR_REPLACED: choose a registered track")
    return DocumentsContract.buildDocumentUriUsingTree(tree, track.getString("documentId"))
  }

  fun verifyResource(bookId: String, trackId: String): Uri {
    val uri: Uri
    val size: Long
    val modified: Long
    synchronized(this) {
      uri = resource(bookId, trackId)
      val tracks = mutableBook(bookId).getJSONArray("tracks")
      val track = (0 until tracks.length()).map { tracks.getJSONObject(it) }.first { it.getString("id") == trackId }
      size = track.getLong("size")
      modified = track.getLong("modified")
    }
    val columns = arrayOf(DocumentsContract.Document.COLUMN_SIZE, DocumentsContract.Document.COLUMN_LAST_MODIFIED)
    context.contentResolver.query(uri, columns, null, null, null)?.use { cursor ->
      require(cursor.moveToFirst()) { "SOURCE_MISSING: rescan or relink this book" }
      require(cursor.getLong(0) == size && cursor.getLong(1) == modified) {
        "SOURCE_REPLACED: rescan and select the replacement explicitly"
      }
    } ?: throw IllegalStateException("SOURCE_MISSING: rescan or relink this book")
    return uri
  }

  @Synchronized fun checkpoint(bookId: String, trackId: String, offsetMs: Long) {
    require(offsetMs >= 0) { "INVALID_POSITION" }
    val book = mutableBook(bookId)
    require(hasTrack(book, trackId)) { "TRACK_MISSING_OR_REPLACED" }
    book.put("position", JSONObject().put("trackId", trackId).put("offsetMs", offsetMs))
    save()
  }

  @Synchronized fun position(bookId: String, locator: Any, offsetMs: Long = 0): JSONObject {
    val canonical = ReaderNarrationUploads.canonicalLocator(locator)
    require(offsetMs >= 0) { "INVALID_POSITION" }
    val book = mutableBook(bookId)
    require(book.getString("kind") == "epub") { "EPUB_REQUIRED" }
    val position = JSONObject().put("locator", canonical).put("offsetMs", offsetMs)
    book.getJSONArray("tracks").optJSONObject(0)?.let { position.put("sourceTrackId", it.getString("id")) }
    book.put("position", position)
    save()
    return book.getJSONObject("position")
  }

  @Synchronized fun bookmark(bookId: String, trackId: String?, offsetMs: Long, note: String): JSONObject {
    require(note.length <= 4096) { "NOTE_TOO_LONG" }
    val book = mutableBook(bookId)
    val position = if (trackId == null) JSONObject(book.getJSONObject("position").toString())
      else JSONObject().put("trackId", trackId).put("offsetMs", offsetMs)
    if (trackId != null) require(hasTrack(book, trackId)) { "TRACK_MISSING_OR_REPLACED" }
    return bookmarkAt(bookId, position, note)
  }

  @Synchronized fun narrationCheckpoint(bookId: String, position: JSONObject) {
    val book = mutableBook(bookId)
    require(book.getString("kind") == "epub" && position.getLong("offsetMs") >= 0) { "INVALID_NARRATION_POSITION" }
    val canonical = JSONObject(position.toString()).put("locator", ReaderNarrationUploads.canonicalLocator(position.get("locator")))
    book.put("position", canonical)
    save()
  }

  @Synchronized fun bookmarkVisual(bookId: String, raw: JSONObject, note: String): JSONObject {
    val book = mutableBook(bookId)
    require(book.getString("kind") == "epub") { "EPUB_REQUIRED" }
    require(raw.keys().asSequence().toSet() == setOf("locator", "offsetMs")) { "INVALID_POSITION" }
    val offset = raw.get("offsetMs")
    require(offset is Number && offset.toDouble() == offset.toLong().toDouble() &&
      offset.toLong() in 0..9_007_199_254_740_991L) { "INVALID_POSITION" }
    val position = JSONObject().put("locator", ReaderNarrationUploads.canonicalLocator(raw.get("locator")))
      .put("offsetMs", offset.toLong())
    return storeBookmark(bookId, position, note, updatePosition = false)
  }

  @Synchronized fun bookmarkAt(bookId: String, position: JSONObject, note: String): JSONObject =
    storeBookmark(bookId, position, note, updatePosition = true)

  private fun storeBookmark(bookId: String, position: JSONObject, note: String, updatePosition: Boolean): JSONObject {
    require(note.length <= 4096) { "NOTE_TOO_LONG" }
    val book = mutableBook(bookId)
    if (book.has("sourceUrl")) externalPosition(book, position)
    if (position.has("locator")) position.put("locator", ReaderNarrationUploads.canonicalLocator(position.get("locator")))
    val mark = JSONObject().put("id", UUID.randomUUID().toString()).put("bookId", bookId).put("createdAt", System.currentTimeMillis())
      .put("position", position).put("note", note)
    val marks = book.getJSONArray("bookmarks")
    require(marks.length() < 50000) { "BOOKMARK_LIMIT: export your private history" }
    marks.put(mark)
    if (updatePosition) book.put("position", position)
    save()
    return JSONObject(mark.toString())
  }

  private fun applyOrder(book: JSONObject, ids: JSONArray) {
    val tracks = book.getJSONArray("tracks")
    val byId = (0 until tracks.length()).associate { tracks.getJSONObject(it).getString("id") to tracks.getJSONObject(it) }.toMutableMap()
    val sorted = JSONArray()
    for (i in 0 until ids.length()) byId.remove(ids.getString(i))?.let { sorted.put(it) }
    byId.values.forEach { sorted.put(it) }
    book.put("tracks", sorted).put("order", ids)
  }

  @Synchronized fun reorder(bookId: String, ids: JSONArray) {
    val book = mutableBook(bookId)
    val desired = (0 until ids.length()).map { ids.getString(it) }
    require(desired.size == book.getJSONArray("tracks").length() && desired.distinct().size == desired.size &&
      desired.all { hasTrack(book, it) }) { "INVALID_TRACK_ORDER" }
    applyOrder(book, ids)
    save()
  }

  @Synchronized fun remove(bookId: String) {
    val book = mutableBook(bookId)
    val excluded = data.optJSONArray("excluded") ?: JSONArray()
    excluded.put(book.optString("bindingId", bookId))
    data.put("excluded", excluded)
    val books = data.getJSONArray("books")
    for (i in books.length() - 1 downTo 0) if (books.getJSONObject(i).getString("id") == bookId) books.remove(i)
    save()
  }

  @Synchronized fun relink(bookId: String, replacementId: String) {
    require(bookId != replacementId) { "CHOOSE_REPLACEMENT_BOOK" }
    val previous = mutableBook(bookId)
    val fresh = JSONObject(mutableBook(replacementId).toString())
    require(!previous.has("sourceUrl") && !fresh.has("sourceUrl")) { "EXTERNAL_LINKS_CANNOT_RELINK" }
    require(!fresh.optBoolean("favorite") && fresh.optLong("lastUsedAt") == 0L && !fresh.has("progress")) {
      "REPLACEMENT_HAS_LIBRARY_HISTORY: keep this item separate"
    }
    require(fresh.optBoolean("available") && previous.getString("kind") == fresh.getString("kind")) { "INCOMPATIBLE_REPLACEMENT" }
    // Explicit relink never applies an old numeric position to potentially different media.
    val marks = JSONArray(previous.getJSONArray("bookmarks").toString())
    val replacementMarks = fresh.getJSONArray("bookmarks")
    for (i in 0 until replacementMarks.length()) marks.put(replacementMarks.getJSONObject(i))
    require(marks.length() <= 50000) { "BOOKMARK_LIMIT: export history before relinking" }
    fresh.put("bindingId", fresh.optString("bindingId", replacementId))
      .put("id", bookId).put("bookmarks", marks)
      .put("previousPosition", previous.getJSONObject("position"))
      .put("position", JSONObject().put("offsetMs", 0))
    for (field in listOf("favorite", "lastUsedAt", "progress")) if (previous.has(field)) fresh.put(field, previous.get(field))
    val result = JSONArray()
    val books = data.getJSONArray("books")
    for (i in 0 until books.length()) {
      val current = books.getJSONObject(i)
      if (current.getString("id") == bookId) result.put(fresh)
      else if (current.getString("id") != replacementId) result.put(current)
    }
    data.put("books", result)
    save()
  }

  @Synchronized fun exportState(): String {
    val json = data.toString()
    require(json.toByteArray().size <= 16 * 1024 * 1024) {
      "BACKUP_SIZE_LIMIT: private history exceeds the 16 MiB restore limit; no backup content was written"
    }
    return json
  }

  @Synchronized fun updateBookmark(bookId: String, bookmarkId: String, note: String): JSONObject {
    require(note.length <= 4096) { "NOTE_TOO_LONG" }
    val marks = mutableBook(bookId).getJSONArray("bookmarks")
    val mark = (0 until marks.length()).map { marks.getJSONObject(it) }
      .find { it.getString("id") == bookmarkId } ?: throw IllegalArgumentException("BOOKMARK_NOT_FOUND")
    mark.put("note", note).put("bookId", bookId)
    save()
    return JSONObject(mark.toString())
  }

  @Synchronized fun deleteBookmark(bookId: String, bookmarkId: String): JSONObject {
    val marks = mutableBook(bookId).getJSONArray("bookmarks")
    val index = (0 until marks.length()).find { marks.getJSONObject(it).getString("id") == bookmarkId }
      ?: throw IllegalArgumentException("BOOKMARK_NOT_FOUND")
    marks.remove(index)
    save()
    return library()
  }

  /** Merge recovery never replaces current progress, grants, settings, or edited notes. */
  fun restore(json: String): JSONObject {
    require(json.toByteArray().size <= 16 * 1024 * 1024) { "RESTORE_SIZE_LIMIT" }
    val source = JSONObject(json)
    ReaderPolicy.libraryVersion(source)
    val imported = source.getJSONArray("books")
    require(imported.length() <= 10000) { "RESTORE_BOOK_LIMIT" }
    val safe = ArrayList<JSONObject>()
    val ids = HashSet<String>()
    var trackCount = 0
    var bookmarkCount = 0
    fun position(raw: JSONObject): JSONObject {
      val offset = raw.optLong("offsetMs", 0)
      require(offset >= 0) { "INVALID_RESTORE_POSITION" }
      val value = JSONObject().put("offsetMs", offset)
      if (raw.has("trackId")) {
        require(Regex("[a-f0-9]{64}").matches(raw.getString("trackId"))) { "INVALID_RESTORE_TRACK_ID" }
        value.put("trackId", raw.getString("trackId"))
      }
      if (raw.has("locator")) {
        value.put("locator", ReaderNarrationUploads.canonicalLocator(raw.get("locator")))
        if (raw.has("ttsOrdinal")) {
          require(raw.getInt("ttsOrdinal") in 0..199999) { "INVALID_RESTORE_ORDINAL" }
          value.put("ttsOrdinal", raw.getInt("ttsOrdinal"))
        }
        for (field in listOf("sourceHash", "sourceTrackId")) if (raw.has(field)) {
          require(Regex("[a-f0-9]{64}").matches(raw.getString(field))) { "INVALID_RESTORE_SOURCE_IDENTITY" }
          value.put(field, raw.getString(field))
        }
        if (raw.has("voiceId")) {
          require(Regex("[A-Za-z0-9_-]{1,96}").matches(raw.getString("voiceId"))) { "INVALID_RESTORE_VOICE_ID" }
          value.put("voiceId", raw.getString("voiceId"))
        }
      }
      return value
    }
    for (i in 0 until imported.length()) {
      val book = imported.getJSONObject(i)
      val id = book.getString("id")
      require(Regex("[a-f0-9]{64}").matches(id) && ids.add(id)) { "INVALID_OR_DUPLICATE_RESTORE_BOOK_ID" }
      val kind = book.getString("kind")
      ReaderPolicy.libraryMetadata(book)
      require(kind in setOf("audio", "epub", "video", "youtube") && book.getString("title").length in 1..1024) { "INVALID_RESTORE_BOOK" }
      val tracks = JSONArray()
      val trackIds = HashSet<String>()
      val incomingTracks = book.getJSONArray("tracks")
      for (j in 0 until incomingTracks.length()) {
        require(++trackCount <= 10000) { "RESTORE_TRACK_LIMIT" }
        val track = incomingTracks.getJSONObject(j)
        val trackId = track.getString("id")
        val relative = track.getString("relativePath")
        require(Regex("[a-f0-9]{64}").matches(trackId) && trackIds.add(trackId) &&
          track.getString("title").length in 1..1024 && relative.length in 1..4096 &&
          !relative.contains('\u0000')) { "INVALID_RESTORE_TRACK" }
        tracks.put(JSONObject().put("id", trackId).put("title", track.getString("title"))
          .put("relativePath", relative))
      }
      val marks = JSONArray()
      val markIds = HashSet<String>()
      val incomingMarks = book.getJSONArray("bookmarks")
      for (j in 0 until incomingMarks.length()) {
        require(++bookmarkCount <= 50000) { "RESTORE_BOOKMARK_LIMIT" }
        val mark = incomingMarks.getJSONObject(j)
        val markId = mark.getString("id")
        require(markId.length in 1..128 && markIds.add(markId) && mark.optString("note").length <= 4096 &&
          mark.getLong("createdAt") >= 0) { "INVALID_RESTORE_BOOKMARK" }
        marks.put(JSONObject().put("id", markId).put("bookId", id).put("createdAt", mark.getLong("createdAt"))
          .put("note", mark.optString("note")).put("position", position(mark.getJSONObject("position"))))
      }
      val restored = JSONObject().put("id", id).put("title", book.getString("title")).put("kind", kind)
        .put("tracks", tracks).put("bookmarks", marks).put("position", position(book.getJSONObject("position")))
        .put("available", false).put("error", "RESTORED_SOURCE_RELINK_REQUIRED: select and rescan the local library")
        .put("order", JSONArray((0 until tracks.length()).map { tracks.getJSONObject(it).getString("id") }))
      for (field in listOf("favorite", "lastUsedAt", "progress")) if (book.has(field)) restored.put(field, book.get(field))
      if (book.has("sourceUrl")) {
        restored.put("sourceUrl", book.getString("sourceUrl")).put("available", true).remove("error")
        externalPosition(restored, restored.getJSONObject("position"))
        for (j in 0 until marks.length()) externalPosition(restored, marks.getJSONObject(j).getJSONObject("position"))
      }
      if (book.has("bindingId")) {
        require(Regex("[a-f0-9]{64}").matches(book.getString("bindingId"))) { "INVALID_RESTORE_BINDING" }
        restored.put("bindingId", book.getString("bindingId"))
      }
      safe.add(restored)
    }
    synchronized(this) {
      val backup = AtomicFile(File(file.baseFile.parentFile, "before-restore-${UUID.randomUUID()}.json"))
      val before = data.toString()
      val output = backup.startWrite()
      try {
        output.write(before.toByteArray())
        backup.finishWrite(output)
        require(backup.openRead().use { it.readBytes() }.contentEquals(before.toByteArray())) { "RESTORE_BACKUP_VERIFICATION_FAILED" }
      }
      catch (error: Exception) { backup.failWrite(output); throw IllegalStateException("RESTORE_BACKUP_FAILED", error) }
      try {
        val current = data.getJSONArray("books")
        for (book in safe) {
          val existing = (0 until current.length()).map { current.getJSONObject(it) }
            .find { it.getString("id") == book.getString("id") }
          if (existing == null) {
            require(current.length() < 10000) { "RESTORE_BOOK_LIMIT" }
            current.put(book)
          } else {
            require(existing.getString("kind") == book.getString("kind") &&
              existing.optString("sourceUrl") == book.optString("sourceUrl")) { "RESTORE_SOURCE_CONFLICT" }
            val marks = existing.getJSONArray("bookmarks")
            val existingIds = (0 until marks.length()).map { marks.getJSONObject(it).getString("id") }.toMutableSet()
            val additions = book.getJSONArray("bookmarks")
            for (i in 0 until additions.length()) {
              val mark = additions.getJSONObject(i)
              if (existingIds.add(mark.getString("id"))) {
                require(marks.length() < 50000) { "RESTORE_BOOKMARK_LIMIT" }
                marks.put(mark)
              }
            }
          }
        }
        save()
      } catch (error: Exception) {
        data = JSONObject(before)
        throw error
      }
      return library().put("restoreMode", "merge")
        .put("restoreNotice", "Existing progress and edited notes retained. Restored missing books require selecting/rescanning local sources; permissions and volume-key opt-in were not restored.")
    }
  }

  companion object {
    @Volatile private var instance: ReaderLibrary? = null
    fun get(context: Context): ReaderLibrary = instance ?: synchronized(this) {
      instance ?: ReaderLibrary(context.applicationContext).also { instance = it }
    }
  }
}
