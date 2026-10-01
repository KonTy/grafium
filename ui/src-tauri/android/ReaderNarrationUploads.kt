package com.grafium.app

import android.content.Context
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.UUID

/** A canonical-CFI queue is published only after all chunks and the source are validated. */
internal class ReaderNarrationUploads private constructor(private val context: Context) {
  private val root = File(context.noBackupFilesDir, "private-reader/voices").apply { mkdirs() }
  private data class Upload(val id: String, val bookId: String, val trackId: String,
    val sourceHash: String, val file: File, var count: Int = 0, var bytes: Long = 0,
    var touchedAt: Long = System.currentTimeMillis())
  private var active: Upload? = null

  init {
    root.listFiles()?.filter { it.isFile && it.name.matches(Regex("narration-upload-[a-f0-9-]{36}\\.json")) }
      ?.forEach { it.delete() }
  }

  @Synchronized fun begin(bookId: String): JSONObject {
    active?.let {
      require(System.currentTimeMillis() - it.touchedAt > 30 * 60 * 1000) { "NARRATION_UPLOAD_IN_PROGRESS: cancel or finish the pending upload" }
      it.file.delete()
      active = null
    }
    val book = ReaderLibrary.get(context).book(bookId)
    require(book.getString("kind") == "epub" && book.optBoolean("available")) { "EPUB_SOURCE_UNAVAILABLE" }
    val track = book.getJSONArray("tracks").getJSONObject(0).getString("id")
    val hash = sourceHash(bookId, track)
    val id = UUID.randomUUID().toString()
    val file = File(root, "narration-upload-$id.json")
    val prefix = """{"book_id":${JSONObject.quote(bookId)},"document":{"version":1,"source_hash":"$hash","segments":["""
      .toByteArray()
    FileOutputStream(file).use { it.write(prefix); it.fd.sync() }
    active = Upload(id, bookId, track, hash, file, bytes = prefix.size.toLong())
    return JSONObject().put("uploadId", id).put("sourceHash", hash)
  }

  @Synchronized fun append(uploadId: String, segments: JSONArray): JSONObject {
    val upload = pending(uploadId)
    require(segments.length() in 1..256) { "NARRATION_CHUNK_SEGMENT_LIMIT" }
    require(segments.toString().toByteArray().size < 512 * 1024) { "NARRATION_CHUNK_SIZE_LIMIT" }
    require(upload.count + segments.length() <= 200000) { "NARRATION_SEGMENT_LIMIT" }
    val encoded = ArrayList<String>()
    for (i in 0 until segments.length()) {
      val segment = segments.getJSONObject(i)
      val expected = upload.count + i
      require(segment.getInt("ordinal") == expected) { "NARRATION_ORDINAL_MISMATCH" }
      val text = segment.getString("text")
      require(text.isNotBlank() && text.toByteArray().size <= 1500 && !text.contains('\u0000')) { "INVALID_NARRATION_TEXT" }
      val locator = validateLocator(segment.getJSONObject("locator"))
      encoded.add(JSONObject().put("ordinal", expected).put("text", text).put("locator", locator).toString())
    }
    val bytes = ((if (upload.count == 0) "" else ",") + encoded.joinToString(",")).toByteArray()
    require(upload.bytes + bytes.size + 3 <= 64L * 1024 * 1024) { "NARRATION_QUEUE_SIZE_LIMIT" }
    try {
      FileOutputStream(upload.file, true).use { it.write(bytes); it.fd.sync() }
    } catch (error: Exception) {
      cancel(uploadId)
      throw IllegalStateException("NARRATION_UPLOAD_WRITE_FAILED", error)
    }
    upload.count += segments.length()
    upload.bytes += bytes.size
    upload.touchedAt = System.currentTimeMillis()
    return JSONObject().put("segmentCount", upload.count)
  }

  @Synchronized fun commit(uploadId: String): JSONObject {
    val upload = pending(uploadId)
    try {
      require(upload.count > 0) { "NARRATION_QUEUE_EMPTY" }
      require(sourceHash(upload.bookId, upload.trackId) == upload.sourceHash) {
        "NARRATION_SOURCE_CHANGED: reopen the book and prepare its full text again"
      }
      FileOutputStream(upload.file, true).use { it.write("]}}".toByteArray()); it.fd.sync() }
      val destination = AtomicFile(File(root, "narration.json"))
      val output = destination.startWrite()
      try {
        upload.file.inputStream().use { it.copyTo(output, 64 * 1024) }
        destination.finishWrite(output)
      } catch (error: Exception) {
        destination.failWrite(output)
        throw IllegalStateException("NARRATION_COMMIT_FAILED", error)
      }
    } catch (error: Exception) {
      cancel(uploadId)
      throw error
    }
    upload.file.delete()
    active = null
    return JSONObject().put("book_id", upload.bookId).put("source_hash", upload.sourceHash)
      .put("segment_count", upload.count)
  }

  @Synchronized fun cancel(uploadId: String): JSONObject {
    val upload = pending(uploadId)
    upload.file.delete()
    active = null
    return JSONObject().put("cancelled", true)
  }

  fun verifyForStart(bookId: String): String {
    val book = ReaderLibrary.get(context).book(bookId)
    require(book.getString("kind") == "epub" && book.optBoolean("available")) { "EPUB_SOURCE_UNAVAILABLE" }
    return sourceHash(bookId, book.getJSONArray("tracks").getJSONObject(0).getString("id"))
  }

  private fun pending(id: String): Upload = active?.takeIf { it.id == id }
    ?: throw IllegalArgumentException("NARRATION_UPLOAD_NOT_FOUND")

  private fun sourceHash(bookId: String, trackId: String): String {
    val uri = ReaderLibrary.get(context).verifyResource(bookId, trackId)
    val digest = MessageDigest.getInstance("SHA-256")
    context.contentResolver.openInputStream(uri)?.use { input ->
      val buffer = ByteArray(64 * 1024)
      var total = 0L
      while (true) {
        val count = input.read(buffer)
        if (count < 0) break
        total += count
        require(total <= 32L * 1024 * 1024) { "EPUB_TOO_LARGE: Android reader limit is 32 MiB" }
        digest.update(buffer, 0, count)
      }
    } ?: throw IllegalStateException("EPUB_SOURCE_UNAVAILABLE")
    return digest.digest().joinToString("") { "%02x".format(it) }
  }

  companion object {
    internal fun resumeOrdinal(segments: JSONArray, saved: JSONObject, sourceHash: String, fromBeginning: Boolean): Int {
      if (fromBeginning || !saved.has("locator")) return 0
      require(!saved.has("sourceHash") || saved.getString("sourceHash") == sourceHash) {
        "NARRATION_SOURCE_CHANGED: choose Read from beginning explicitly"
      }
      val locator = try { canonicalLocator(saved.get("locator")) }
        catch (_: Exception) { throw IllegalArgumentException("NARRATION_LOCATION_NOT_IN_QUEUE: choose Read from beginning explicitly") }
      fun matches(index: Int): Boolean {
        val candidate = segments.getJSONObject(index).getJSONObject("locator")
        return candidate.optString("kind") == "epub" && candidate.optString("cfi") == locator.getString("cfi") &&
          candidate.optString("rendererVersion") == locator.getString("rendererVersion")
      }
      val hint = saved.optInt("ttsOrdinal", -1)
      if (saved.optString("sourceHash") == sourceHash && hint in 0 until segments.length() && matches(hint)) return hint
      val matching = (0 until segments.length()).filter(::matches)
      require(matching.size == 1) { "NARRATION_LOCATION_NOT_IN_QUEUE: choose Read from beginning explicitly" }
      return matching.single()
    }

    internal fun canonicalLocator(raw: Any?): JSONObject {
      val value = when (raw) {
        is JSONObject -> raw
        is String -> try { JSONObject(raw) } catch (_: Exception) {
          throw IllegalArgumentException("INVALID_CANONICAL_EPUB_LOCATOR")
        }
        else -> throw IllegalArgumentException("INVALID_CANONICAL_EPUB_LOCATOR")
      }
      return validateLocator(value)
    }

    internal fun validateLocator(raw: JSONObject): JSONObject {
      val cfi = raw.getString("cfi")
      val renderer = raw.getString("rendererVersion")
      require(raw.getString("kind") == "epub" && cfi.length in 10..8192 &&
        cfi.startsWith("epubcfi(") && cfi.endsWith(")") &&
        !cfi.any { it.code < 32 } && renderer.length in 1..128 &&
        !renderer.any { it.code < 32 }) { "INVALID_CANONICAL_EPUB_LOCATOR" }
      return JSONObject().put("kind", "epub").put("cfi", cfi).put("rendererVersion", renderer)
    }
    @Volatile private var instance: ReaderNarrationUploads? = null
    fun get(context: Context): ReaderNarrationUploads = instance ?: synchronized(this) {
      instance ?: ReaderNarrationUploads(context.applicationContext).also { instance = it }
    }
  }
}
