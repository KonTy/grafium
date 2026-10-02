package com.grafium.app

import android.content.Context
import android.content.ContextWrapper
import android.os.Looper
import android.webkit.ValueCallback
import android.webkit.WebView
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows
import org.robolectric.annotation.Config
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], manifest = Config.NONE)
class ReaderSharedContractTest {
  private val bookId = ReaderPolicy.stableId("shared-contract-epub")
  private fun fixture(): JSONObject = javaClass.classLoader!!
    .getResourceAsStream("private-reader-position.json")!!.bufferedReader().use { JSONObject(it.readText()) }
  private fun open(context: Context): ReaderLibrary = ReaderLibrary::class.java.getDeclaredConstructor(Context::class.java)
    .apply { isAccessible = true }.newInstance(context)
  private fun useLibrary(library: ReaderLibrary?) {
    ReaderLibrary::class.java.getDeclaredField("instance").apply { isAccessible = true }.set(null, library)
  }
  private fun seed(context: Context): ReaderLibrary {
    val book = JSONObject().put("id", bookId).put("title", "Shared fixture EPUB").put("kind", "epub")
      .put("available", true).put("tracks", JSONArray()).put("bookmarks", JSONArray())
      .put("position", JSONObject().put("offsetMs", 0))
    val file = File(context.noBackupFilesDir, "private-reader/library.json")
    file.parentFile!!.mkdirs()
    file.writeText(JSONObject().put("books", JSONArray().put(book)).put("volume",
      JSONObject().put("enabled", false).put("key", "up").put("gesture", "longPress")).toString())
    return open(context)
  }
  private fun freshContext(context: Context, name: String): Context = object : ContextWrapper(context) {
    override fun getNoBackupFilesDir(): File = File(context.noBackupFilesDir, name).apply { mkdirs() }
  }
  private fun assertPosition(expected: JSONObject, actual: JSONObject) {
    assertEquals(expected.getLong("offsetMs"), actual.getLong("offsetMs"))
    val locator = actual.getJSONObject("locator")
    val source = expected.getJSONObject("locator")
    assertEquals(source.keys().asSequence().toSet(), locator.keys().asSequence().toSet())
    assertEquals("epub", locator.getString("kind"))
    assertEquals(source.getString("cfi"), locator.getString("cfi"))
    assertEquals(source.getString("rendererVersion"), locator.getString("rendererVersion"))
    assertFalse(locator.has("type"))
  }

  @Test fun sharedFixturePersistsBookmarksAndRoundtripsThroughRealBackup() {
    val context = RuntimeEnvironment.getApplication()
    val expected = fixture()
    val library = seed(context)
    library.position(bookId, expected.getJSONObject("locator"), expected.getLong("offsetMs"))
    assertPosition(expected, open(context).book(bookId).getJSONObject("position"))
    val bookmark = library.bookmark(bookId, null, 0, "Shared fixture")
    assertPosition(expected, bookmark.getJSONObject("position"))
    val exported = library.exportState()
    assertPosition(expected, JSONObject(exported).getJSONArray("books").getJSONObject(0).getJSONObject("position"))
    val targetContext = freshContext(context, "shared-contract-restore")
    open(targetContext).restore(exported)
    val restored = open(targetContext).book(bookId)
    assertPosition(expected, restored.getJSONObject("position"))
    assertPosition(expected, restored.getJSONArray("bookmarks").getJSONObject(0).getJSONObject("position"))
  }

  @Test fun actualBridgeSaveBookmarkRestoreAndJavascriptResponsePreserveFixture() {
    val context = RuntimeEnvironment.getApplication()
    val expected = fixture()
    val source = seed(context)
    useLibrary(source)
    val controller = Robolectric.buildActivity(MainActivity::class.java)
    controller.get().setTheme(androidx.appcompat.R.style.Theme_AppCompat)
    val activity = controller.create().start().resume().get()
    val scripts = CopyOnWriteArrayList<String>()
    val view = object : WebView(activity) {
      override fun evaluateJavascript(script: String, resultCallback: ValueCallback<String>?) {
        scripts.add(script)
      }
    }
    view.loadUrl("https://tauri.localhost")
    val bridge = PrivateReaderBridge(activity, view, {}, {}, {})
    val accept = PrivateReaderBridge::class.java.getDeclaredMethod("accept", String::class.java).apply { isAccessible = true }
    var serial = 0
    fun command(name: String, args: JSONObject): JSONObject {
      val id = "shared-fixture-${serial++}"
      accept.invoke(bridge, JSONObject().put("id", id).put("command", name).put("args", args).toString())
      repeat(400) {
        Shadows.shadowOf(Looper.getMainLooper()).idle()
        for (script in scripts) {
          val encoded = script.substringAfter("JSON.parse(", "").substringBeforeLast(")}));", "")
          if (encoded.isEmpty()) continue
          val response = JSONObject(JSONTokener(encoded).nextValue() as String)
          if (response.optString("id") == id) return response
        }
        Thread.sleep(5)
      }
      throw AssertionError("Native bridge response timed out for $name")
    }
    try {
      val saved = command("position", JSONObject().put("bookId", bookId)
        .put("locator", expected.getJSONObject("locator")).put("offsetMs", expected.getLong("offsetMs")))
      assertTrue(saved.toString(), saved.getBoolean("ok"))
      assertPosition(expected, saved.getJSONObject("result"))
      val bookmarked = command("bookmark", JSONObject().put("bookId", bookId).put("note", "Through actual bridge"))
      assertTrue(bookmarked.toString(), bookmarked.getBoolean("ok"))
      assertPosition(expected, bookmarked.getJSONObject("result").getJSONObject("position"))
      val backup = source.exportState()
      useLibrary(open(freshContext(context, "shared-bridge-restore")))
      val restored = command("restore", JSONObject().put("data", backup))
      assertTrue(restored.toString(), restored.getBoolean("ok"))
      val book = restored.getJSONObject("result").getJSONArray("books").getJSONObject(0)
      assertPosition(expected, book.getJSONObject("position"))
      assertPosition(expected, book.getJSONArray("bookmarks").getJSONObject(0).getJSONObject("position"))
      val legacy = JSONObject(expected.getJSONObject("locator").toString()).put("type", "epub").apply { remove("kind") }
      val rejected = command("position", JSONObject().put("bookId", bookId).put("locator", legacy)
        .put("offsetMs", expected.getLong("offsetMs")))
      assertFalse(rejected.getBoolean("ok"))
      assertPosition(expected, ReaderLibrary.get(context).book(bookId).getJSONObject("position"))
      val abort = command("narrationAbort", JSONObject().put("uploadId", "not-an-active-upload"))
      assertFalse(abort.getBoolean("ok"))
      assertEquals("NARRATION_UPLOAD_NOT_FOUND", abort.getString("error"))
      val added = command("add_link", JSONObject().put("title", "Private talk").put("kind", "video")
        .put("url", "https://media.example/talk.mp4"))
      assertTrue(added.toString(), added.getBoolean("ok"))
      val link = added.getJSONObject("result").getJSONArray("books").getJSONObject(1)
      val linkId = link.getString("id")
      val favorite = command("set_favorite", JSONObject().put("bookId", linkId).put("favorite", true))
      assertTrue(favorite.toString(), favorite.getBoolean("ok"))
      assertTrue(favorite.getJSONObject("result").getJSONArray("books").getJSONObject(1).getBoolean("favorite"))
      assertEquals(0, favorite.getJSONObject("result").getJSONArray("books").getJSONObject(1).getLong("lastUsedAt"))
      val recorded = command("record_activity", JSONObject().put("bookId", linkId).put("progress",
        JSONObject().put("position", 8).put("total", 80).put("anchor", "").put("label", "0:08")))
      assertTrue(recorded.toString(), recorded.getBoolean("ok"))
      assertTrue(recorded.getJSONObject("result").getJSONArray("books").getJSONObject(1).getLong("lastUsedAt") > 0)
      val linkPosition = JSONObject().put("bookId", linkId).put("position", JSONObject().put("offsetMs", 8000))
      assertTrue(command("external_position", linkPosition).getBoolean("ok"))
      val externalBookmark = command("external_bookmark", linkPosition.put("note", "At eight seconds"))
      assertTrue(externalBookmark.toString(), externalBookmark.getBoolean("ok"))
      assertEquals(8000, externalBookmark.getJSONObject("result").getJSONObject("position").getLong("offsetMs"))
    } finally {
      bridge.destroy()
      view.destroy()
      controller.pause().stop().destroy()
      useLibrary(null)
    }
  }
}
