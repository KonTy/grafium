package com.grafium.app

import android.content.Context
import android.content.ContextWrapper
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], manifest = Config.NONE)
class ReaderPersistenceTest {
  private lateinit var context: Context
  private fun open(): ReaderLibrary = ReaderLibrary::class.java.getDeclaredConstructor(Context::class.java)
    .apply { isAccessible = true }.newInstance(context)

  @Before fun seedPrivateRegistry() {
    context = RuntimeEnvironment.getApplication()
    val books = JSONArray().put(JSONObject().put("id", "book-a").put("title", "Book A")
      .put("kind", "audio").put("available", true).put("tracks", JSONArray()
        .put(JSONObject().put("id", "chapter-1").put("title", "Chapter 1").put("relativePath", "Chapter 1.mp3"))
        .put(JSONObject().put("id", "chapter-2").put("title", "Chapter 2").put("relativePath", "Chapter 2.mp3")))
      .put("position", JSONObject().put("offsetMs", 0)).put("bookmarks", JSONArray()))
      .put(JSONObject().put("id", "book-b").put("title", "Book B").put("kind", "epub")
        .put("available", true).put("tracks", JSONArray()).put("bookmarks", JSONArray())
        .put("position", JSONObject().put("offsetMs", 0)))
    val file = File(context.noBackupFilesDir, "private-reader/library.json")
    file.parentFile!!.mkdirs()
    file.writeText(JSONObject().put("books", books).put("volume", JSONObject()
      .put("enabled", false).put("key", "up").put("gesture", "longPress")).toString())
  }

  @Test fun checkpointsAndImmediateBookmarksSurviveRegistryRecreation() {
    val library = open()
    library.checkpoint("book-a", "chapter-2", 42001)
    val mark = library.bookmark("book-a", "chapter-2", 43002, "quote ' newline\n")
    val reopened = open().book("book-a")
    assertEquals("chapter-2", reopened.getJSONObject("position").getString("trackId"))
    assertEquals(43002, reopened.getJSONObject("position").getLong("offsetMs"))
    assertEquals(mark.toString(), reopened.getJSONArray("bookmarks").getJSONObject(0).toString())
    assertEquals(0, open().book("book-b").getJSONObject("position").getLong("offsetMs"))
    assertTrue(File(context.noBackupFilesDir, "private-reader/library.json").isFile)
    assertFalse(File(context.filesDir, "private-reader/library.json").exists())
  }

  @Test fun libraryMetadataDefaultsAndExplicitActivitySurviveLegacyWrites() {
    val library = open()
    assertFalse(library.book("book-a").getBoolean("favorite"))
    assertEquals(0, library.book("book-a").getLong("lastUsedAt"))
    library.setFavorite("book-a", true)
    assertEquals(0, library.book("book-a").getLong("lastUsedAt"))
    val progress = JSONObject().put("position", 42).put("total", 100).put("anchor", "").put("label", "0:42")
    library.recordActivity("book-a", progress)
    val used = library.book("book-a").getLong("lastUsedAt")
    assertTrue(used > 0)
    library.checkpoint("book-a", "chapter-1", 42000)
    library.bookmark("book-a", "chapter-1", 43000, "Private")
    val reopened = open().book("book-a")
    assertTrue(reopened.getBoolean("favorite"))
    assertEquals(used, reopened.getLong("lastUsedAt"))
    assertEquals(progress.toString(), reopened.getJSONObject("progress").toString())
    val before = library.exportState()
    assertThrows(IllegalArgumentException::class.java) {
      library.recordActivity("book-a", JSONObject(progress.toString()).put("position", -1))
    }
    assertEquals(before, library.exportState())
  }

  @Test fun externalLinksPersistWithoutSourceGrantsAndRestoreTheirHistory() {
    val library = open()
    library.addLink("Talk", "youtube", "https://youtu.be/abcdefghijk?t=5")
    library.addLink("Duplicate", "youtube", "https://youtube.com/embed/abcdefghijk")
    assertEquals(3, library.books().length())
    val link = library.books().getJSONObject(2)
    val id = link.getString("id")
    assertEquals("https://www.youtube.com/watch?v=abcdefghijk", link.getString("sourceUrl"))
    assertFalse(link.has("tree"))
    assertEquals(0, link.getJSONArray("tracks").length())
    library.setFavorite(id, true)
    library.recordActivity(id, JSONObject().put("position", 5).put("total", 50).put("anchor", "").put("label", "0:05"))
    library.saveExternalPosition(id, JSONObject().put("offsetMs", 5000))
    library.externalBookmark(id, JSONObject().put("offsetMs", 5000), "At five seconds")
    assertThrows(IllegalArgumentException::class.java) { library.relink(id, "book-a") }
    assertThrows(IllegalArgumentException::class.java) { library.resource(id, "anything") }
    val exportedLink = JSONObject(library.exportState()).getJSONArray("books").getJSONObject(2)
    val backup = JSONObject().put("books", JSONArray().put(exportedLink)).toString()
    val originalContext = context
    context = object : ContextWrapper(originalContext) {
      override fun getNoBackupFilesDir(): File = File(originalContext.noBackupFilesDir, "external-target").apply { mkdirs() }
    }
    open().restore(backup)
    val restored = open().book(id)
    assertTrue(restored.getBoolean("favorite") && restored.getBoolean("available"))
    assertTrue(restored.getLong("lastUsedAt") > 0)
    assertEquals(5000, restored.getJSONObject("position").getLong("offsetMs"))
    assertEquals(1, restored.getJSONArray("bookmarks").length())
    assertFalse(restored.has("tree"))
  }

  @Test fun invalidExternalLinksAndMetadataNeverChangePrivateState() {
    val library = open()
    val before = library.exportState()
    for (url in listOf("file:///secret", "https://user:pass@example.com/a", "https://@example.com/a",
      "https://example.com/\nfile", "https://example.com\\@evil.test/a")) {
      assertThrows(IllegalArgumentException::class.java) { library.addLink("Bad", "video", url) }
    }
    for (url in listOf("https://youtube.com.evil.test/watch?v=abcdefghijk", "https://youtu.be/short")) {
      assertThrows(IllegalArgumentException::class.java) { library.addLink("Bad", "youtube", url) }
    }
    assertEquals(before, library.exportState())
    val file = File(context.noBackupFilesDir, "private-reader/library.json")
    val invalid = JSONObject(file.readText())
    invalid.getJSONArray("books").getJSONObject(0).put("favorite", "yes")
    file.writeText(invalid.toString())
    assertThrows(Exception::class.java) { open() }
    assertEquals(invalid.toString(), file.readText())
  }

  @Test fun reorderNeverChangesSavedTrackIdentity() {
    val library = open()
    library.checkpoint("book-a", "chapter-2", 9200)
    library.reorder("book-a", JSONArray().put("chapter-2").put("chapter-1"))
    val reopened = open().book("book-a")
    assertEquals("chapter-2", reopened.getJSONObject("position").getString("trackId"))
    assertEquals("chapter-2", reopened.getJSONArray("tracks").getJSONObject(0).getString("id"))
    assertThrows(IllegalArgumentException::class.java) {
      library.reorder("book-a", JSONArray().put("chapter-1").put("chapter-1"))
    }
  }

  @Test fun unknownTrackAndInvalidOffsetsNeverOverwriteSavedProgress() {
    val library = open()
    library.checkpoint("book-a", "chapter-1", 1234)
    assertThrows(IllegalArgumentException::class.java) { library.checkpoint("book-a", "replaced", 0) }
    assertThrows(IllegalArgumentException::class.java) { library.checkpoint("book-a", "chapter-1", -1) }
    assertThrows(IllegalArgumentException::class.java) { library.bookmark("book-a", "replaced", 0, "") }
    val reopened = open().book("book-a")
    assertEquals(1234, reopened.getJSONObject("position").getLong("offsetMs"))
    assertEquals(0, reopened.getJSONArray("bookmarks").length())
  }

  @Test fun epubLocatorsAndVolumeOptInPersistOutsideGraphs() {
    val library = open()
    val locator = JSONObject().put("kind", "epub").put("cfi", "epubcfi(/6/2!/4/2)").put("rendererVersion", "test-renderer")
    library.position("book-b", locator)
    library.bookmark("book-b", null, 0, "")
    library.setVolume(JSONObject().put("enabled", true).put("key", "down").put("gesture", "longPress"))
    assertEquals("epubcfi(/6/2!/4/2)", open().book("book-b").getJSONArray("bookmarks")
      .getJSONObject(0).getJSONObject("position").getJSONObject("locator").getString("cfi"))
    assertEquals("epub", open().book("book-b").getJSONObject("position").getJSONObject("locator").getString("kind"))
    assertEquals("test-renderer", open().book("book-b").getJSONObject("position").getJSONObject("locator").getString("rendererVersion"))
    assertThrows(IllegalArgumentException::class.java) { library.position("book-b", "epubcfi(/6/2!/4/2)") }
    assertEquals("down", open().volume().getString("key"))
    assertThrows(IllegalArgumentException::class.java) {
      library.setVolume(JSONObject().put("enabled", true).put("key", "up").put("gesture", "volumeObserver"))
    }
  }

  @Test fun bookmarkNoteEditsAreDurable() {
    val library = open()
    val mark = library.bookmark("book-a", "chapter-1", 200, "")
    library.updateBookmark("book-a", mark.getString("id"), "My private note")
    assertEquals("My private note", open().book("book-a").getJSONArray("bookmarks").getJSONObject(0).getString("note"))
    assertThrows(IllegalArgumentException::class.java) { library.updateBookmark("book-a", "missing", "note") }
  }

  @Test fun restoreMergesHistoryWithoutRestoringGrantsOrOverwritingNewProgress() {
    val library = open()
    val id = ReaderPolicy.stableId("restored-book")
    val trackId = ReaderPolicy.stableId("restored-track")
    val position = JSONObject().put("trackId", trackId).put("offsetMs", 42)
    val book = JSONObject().put("id", id).put("title", "Restored").put("kind", "audio")
      .put("available", true).put("tree", "content://untrusted.example/tree/secret")
      .put("position", position).put("tracks", JSONArray().put(JSONObject().put("id", trackId)
        .put("title", "Chapter").put("relativePath", "chapter.mp3").put("documentId", "untrusted-document")))
      .put("bookmarks", JSONArray().put(JSONObject().put("id", "mark").put("createdAt", 123456)
        .put("position", position).put("note", "Original")))
    val backup = JSONObject().put("books", JSONArray().put(book))
      .put("volume", JSONObject().put("enabled", true)).toString()
    library.restore(backup)
    assertFalse(open().book(id).getBoolean("available"))
    assertFalse(open().book(id).has("tree"))
    assertFalse(open().volume().getBoolean("enabled"))
    assertEquals("Book A", open().book("book-a").getString("title"))
    library.checkpoint(id, trackId, 900)
    library.updateBookmark(id, "mark", "Edited after backup")
    library.restore(backup)
    val reopened = open().book(id)
    assertEquals(900, reopened.getJSONObject("position").getLong("offsetMs"))
    assertEquals(1, reopened.getJSONArray("bookmarks").length())
    assertEquals("Edited after backup", reopened.getJSONArray("bookmarks").getJSONObject(0).getString("note"))
    assertTrue(File(context.noBackupFilesDir, "private-reader").listFiles()!!.any { it.name.startsWith("before-restore-") })
  }

  @Test fun malformedRestoreLeavesExistingStateIntact() {
    val library = open()
    val before = library.exportState()
    assertThrows(IllegalArgumentException::class.java) {
      library.restore("""{"books":[{"id":"not-a-native-id"}]}""")
    }
    assertEquals(before, library.exportState())
  }

  @Test fun ownExportLargerThanOneMiBRoundtripsWithoutGrantsOrLostHistory() {
    val bookId = ReaderPolicy.stableId("roundtrip-book")
    val trackId = ReaderPolicy.stableId("roundtrip-track")
    val position = JSONObject().put("trackId", trackId).put("offsetMs", 9876)
    val marks = JSONArray()
    val note = "A private bookmark passage. ".repeat(60)
    for (i in 0 until 900) marks.put(JSONObject().put("id", "bookmark-$i").put("bookId", bookId)
      .put("createdAt", 1700000000000L + i).put("position", position).put("note", note))
    val exportedBook = JSONObject().put("id", bookId).put("title", "Roundtrip audiobook").put("kind", "audio")
      .put("tree", "content://com.android.externalstorage.documents/tree/primary%3ABooks")
      .put("documentId", "primary:Books/roundtrip.mp3").put("available", true)
      .put("position", position).put("bookmarks", marks)
      .put("tracks", JSONArray().put(JSONObject().put("id", trackId).put("title", "Chapter")
        .put("relativePath", "roundtrip.mp3").put("documentId", "primary:Books/roundtrip.mp3")
        .put("size", 123).put("modified", 456)))
    val state = JSONObject().put("books", JSONArray().put(exportedBook))
      .put("volume", JSONObject().put("enabled", false).put("key", "up").put("gesture", "longPress"))
    File(context.noBackupFilesDir, "private-reader/library.json").writeText(state.toString())
    val exported = open().exportState()
    assertTrue(exported.toByteArray().size > 1024 * 1024)
    assertTrue(exported.toByteArray().size <= 16 * 1024 * 1024)
    val originalContext = context
    context = object : ContextWrapper(originalContext) {
      override fun getNoBackupFilesDir(): File = File(originalContext.noBackupFilesDir, "roundtrip-target").apply { mkdirs() }
    }
    val target = open()
    target.restore(exported)
    val recovered = open().book(bookId)
    assertEquals(9876, recovered.getJSONObject("position").getLong("offsetMs"))
    assertEquals(trackId, recovered.getJSONObject("position").getString("trackId"))
    assertEquals(900, recovered.getJSONArray("bookmarks").length())
    assertEquals(note, recovered.getJSONArray("bookmarks").getJSONObject(899).getString("note"))
    assertEquals("bookmark-899", recovered.getJSONArray("bookmarks").getJSONObject(899).getString("id"))
    assertFalse(recovered.getBoolean("available"))
    assertFalse(recovered.has("tree"))
    val beforeCorruptRestore = target.exportState()
    assertThrows(Exception::class.java) { target.restore(exported.dropLast(10)) }
    assertEquals(beforeCorruptRestore, target.exportState())
  }
}
