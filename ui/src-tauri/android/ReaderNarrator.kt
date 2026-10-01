package com.grafium.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicLong

/** Two-segment lookahead; all future chapter text is native, not dependent on a WebView timer. */
internal class ReaderNarrator(
  context: Context,
  private val ready: (Int, File, JSONObject, Boolean) -> Unit,
  private val failed: (String) -> Unit
) {
  private val root = File(context.noBackupFilesDir, "private-reader/voices")
  private val cacheRoot = File(root, "narration-audio").apply {
    mkdirs()
    synchronized(ReaderNarrator::class.java) {
      if (!cleanedOnThisProcess) {
        listFiles()?.filter { it.isDirectory && it.name.matches(Regex("[a-f0-9-]{36}")) }
          ?.forEach { it.deleteRecursively() }
        cleanedOnThisProcess = true
      }
    }
  }
  private val cache = File(cacheRoot, java.util.UUID.randomUUID().toString()).apply { mkdirs() }
  private val speech = ReaderSpeechEngine(context)
  private val worker = Executors.newSingleThreadExecutor()
  private val main = Handler(Looper.getMainLooper())
  private val generation = AtomicLong()
  private var segments = JSONArray()
  private val scheduled = HashSet<Int>()
  var count = 0
    private set
  var sourceHash = ""
    private set
  var voiceId = ""
    private set
  private var activeOrdinal = 0

  fun start(bookId: String, saved: JSONObject, fromBeginning: Boolean, verifiedSourceHash: String): Int {
    val queueFile = File(root, "narration.json")
    require(queueFile.isFile && queueFile.length() <= 64L * 1024 * 1024) { "PREPARE_NARRATION_FIRST" }
    val queue = JSONObject(android.util.AtomicFile(queueFile).openRead().bufferedReader().use { it.readText() })
    require(queue.getString("book_id") == bookId) { "PREPARE_NARRATION_FIRST" }
    sourceHash = queue.getJSONObject("document").getString("source_hash")
    require(sourceHash == verifiedSourceHash) { "NARRATION_SOURCE_CHANGED: prepare the full text again" }
    segments = queue.getJSONObject("document").getJSONArray("segments")
    count = segments.length()
    require(count in 1..200000) { "INVALID_NARRATION_POSITION" }
    val ordinal = ReaderNarrationUploads.resumeOrdinal(segments, saved, sourceHash, fromBeginning)
    voiceId = speech.selectedVoiceId() ?: throw IllegalStateException("OFFLINE_VOICE_REQUIRED")
    scheduled.clear()
    activeOrdinal = ordinal
    generate(ordinal, true)
    generate(ordinal + 1, false)
    return ordinal
  }

  private fun generate(ordinal: Int, first: Boolean) {
    if (ordinal >= count || !scheduled.add(ordinal)) return
    val segment = segments.getJSONObject(ordinal)
    val text = segment.getString("text")
    val locator = ReaderNarrationUploads.validateLocator(segment.getJSONObject("locator"))
    require(segment.getInt("ordinal") == ordinal && text.toByteArray().size <= 1500 &&
      locator.toString().length <= 8192) { "INVALID_NARRATION_SEGMENT" }
    val epoch = generation.get()
    worker.execute {
      val file = File(cache, "$epoch-$ordinal.wav")
      try {
        if (generation.get() != epoch) return@execute
        speech.synthesize(text, file) { generation.get() != epoch }
        main.post {
          if (generation.get() == epoch) ready(ordinal, file, locator, first)
          else file.delete()
        }
      } catch (_: Exception) {
        file.delete()
        if (generation.get() == epoch) main.post {
          if (generation.get() == epoch) failed("OFFLINE_SYNTHESIS_FAILED: verify the installed voice package; no cloud or system fallback was used")
        }
      } catch (_: LinkageError) {
        if (generation.get() == epoch) main.post {
          if (generation.get() == epoch) failed("OFFLINE_RUNTIME_UNAVAILABLE: this device's native runtime could not load")
        }
      }
    }
  }

  fun advanced(ordinal: Int) {
    activeOrdinal = ordinal
    generate(ordinal + 1, false)
    // Playback has released older sources by the time the next segment transitions.
    worker.execute {
      cache.listFiles()?.filter { it.name.substringAfter('-').substringBefore('.').toIntOrNull()?.let { n -> n < ordinal - 1 } == true }
        ?.forEach { it.delete() }
    }
  }
  fun locator(ordinal: Int): JSONObject = segments.getJSONObject(ordinal).getJSONObject("locator")
  fun close() {
    generation.incrementAndGet()
    worker.execute {
      speech.release()
      cache.listFiles()?.filter { it.isFile && it.extension == "wav" }?.forEach { it.delete() }
      cache.delete()
    }
    worker.shutdown()
  }
  companion object { private var cleanedOnThisProcess = false }
}
