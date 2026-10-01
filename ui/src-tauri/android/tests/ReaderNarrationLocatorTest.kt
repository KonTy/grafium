package com.grafium.app

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], manifest = Config.NONE)
class ReaderNarrationLocatorTest {
  private fun locator(cfi: String = "epubcfi(/6/2!/4/2)") = JSONObject().put("kind", "epub")
    .put("cfi", cfi).put("rendererVersion", "foliate-test")
  private fun segments() = JSONArray()
    .put(JSONObject().put("ordinal", 0).put("text", "First").put("locator", locator()))
    .put(JSONObject().put("ordinal", 1).put("text", "Second").put("locator", locator("epubcfi(/6/4!/4/2)")))

  @Test fun validatesCanonicalLocatorWithoutInventingCfiFromNativePaths() {
    assertEquals("epub", ReaderNarrationUploads.validateLocator(locator()).getString("kind"))
    assertThrows(Exception::class.java) {
      ReaderNarrationUploads.validateLocator(JSONObject().put("version", 1).put("source_hash", "a")
        .put("spine_index", 0).put("element_path", JSONArray().put(1)))
    }
    assertThrows(IllegalArgumentException::class.java) {
      ReaderNarrationUploads.validateLocator(locator().put("kind", "pdf"))
    }
    assertThrows(IllegalArgumentException::class.java) {
      ReaderNarrationUploads.validateLocator(locator("javascript:alert(1)"))
    }
  }
  @Test fun resumesOnlyExactCfiAndRendererMatch() {
    val saved = JSONObject().put("locator", locator("epubcfi(/6/4!/4/2)").toString()).put("offsetMs", 123)
    assertEquals(1, ReaderNarrationUploads.resumeOrdinal(segments(), saved, "source", false))
    saved.put("locator", locator("epubcfi(/6/4!/4/2)").put("rendererVersion", "other-renderer").toString())
    assertThrows(IllegalArgumentException::class.java) {
      ReaderNarrationUploads.resumeOrdinal(segments(), saved, "source", false)
    }
    assertEquals(0, ReaderNarrationUploads.resumeOrdinal(segments(), saved, "source", true))
  }
  @Test fun sourceReplacementAndNonsegmentLocationsRequireExplicitRestart() {
    val saved = JSONObject().put("locator", locator().toString()).put("sourceHash", "old")
    assertThrows(IllegalArgumentException::class.java) {
      ReaderNarrationUploads.resumeOrdinal(segments(), saved, "new", false)
    }
    assertEquals(0, ReaderNarrationUploads.resumeOrdinal(segments(), saved, "new", true))
    saved.remove("sourceHash")
    saved.put("locator", locator("epubcfi(/6/8!/4/2)").toString())
    assertThrows(IllegalArgumentException::class.java) {
      ReaderNarrationUploads.resumeOrdinal(segments(), saved, "new", false)
    }
  }
  @Test fun duplicateLocatorsNeedMatchingDurableOrdinalAndSource() {
    val queue = segments().put(JSONObject().put("ordinal", 2).put("text", "More").put("locator", locator()))
    val saved = JSONObject().put("locator", locator().toString())
    assertThrows(IllegalArgumentException::class.java) {
      ReaderNarrationUploads.resumeOrdinal(queue, saved, "source", false)
    }
    saved.put("sourceHash", "source").put("ttsOrdinal", 2)
    assertEquals(2, ReaderNarrationUploads.resumeOrdinal(queue, saved, "source", false))
  }
}
