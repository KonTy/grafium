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

  @Test fun bridgeOriginsAreOnesAndroidWebViewAccepts() {
    val accepted = Regex("https?://[A-Za-z0-9.-]+(:[0-9]+)?")
    for (debug in listOf(false, true)) {
      val origins = ReaderPolicy.bridgeOrigins(debug)
      assertTrue(origins.containsAll(listOf("https://tauri.localhost", "http://tauri.localhost")))
      assertTrue(origins.toString(), origins.all { accepted.matches(it) })
    }
    assertFalse(ReaderPolicy.bridgeOrigins(false).any { it.contains("5173") })
  }

  @Test fun onlyExpectedLocalFormatsAreAudio() {
    assertTrue("m4b" in ReaderPolicy.audioExtensions)
    assertFalse("url" in ReaderPolicy.audioExtensions)
    assertFalse("html" in ReaderPolicy.audioExtensions)
    assertFalse("epub" in ReaderPolicy.audioExtensions)
  }
}
