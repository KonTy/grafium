package com.grafium.app

import org.junit.Assert.*
import org.junit.Test

class ReaderPolicyTest {
  @Test fun ordersDiscsAndChaptersNaturally() {
    val paths = listOf("Disc 2/Chapter 10.mp3", "Disc 1/Chapter 10.mp3",
      "Disc 2/Chapter 2.mp3", "Disc 1/Chapter 2.mp3", "Disc 1/Chapter 1.mp3")
    assertEquals(listOf("Disc 1/Chapter 1.mp3", "Disc 1/Chapter 2.mp3", "Disc 1/Chapter 10.mp3",
      "Disc 2/Chapter 2.mp3", "Disc 2/Chapter 10.mp3"), paths.sortedBy(ReaderPolicy::naturalKey))
  }

  @Test fun fingerprintsDoNotSilentlyReuseReplacedTrackIds() {
    val first = ReaderPolicy.stableId("tree|document|1000|1700000000")
    assertEquals(first, ReaderPolicy.stableId("tree|document|1000|1700000000"))
    assertNotEquals(first, ReaderPolicy.stableId("tree|document|2000|1700000000"))
    assertNotEquals(first, ReaderPolicy.stableId("tree|document|1000|1700000001"))
    assertEquals(64, first.length)
    assertTrue(first.matches(Regex("[a-f0-9]+")))
  }

  @Test fun onlyExpectedLocalFormatsAreAudio() {
    assertTrue("m4b" in ReaderPolicy.audioExtensions)
    assertFalse("url" in ReaderPolicy.audioExtensions)
    assertFalse("html" in ReaderPolicy.audioExtensions)
    assertFalse("epub" in ReaderPolicy.audioExtensions)
  }
}
