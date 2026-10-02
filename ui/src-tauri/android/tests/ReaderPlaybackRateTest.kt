package com.grafium.app

import android.content.Context
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.android.controller.ServiceController
import org.robolectric.annotation.Config
import java.io.File

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], manifest = Config.NONE)
class ReaderPlaybackRateTest {
  private lateinit var controller: ServiceController<PrivateReaderService>
  private lateinit var service: PrivateReaderService
  private lateinit var player: ExoPlayer
  private fun field(name: String) = PrivateReaderService::class.java.getDeclaredField(name).apply { isAccessible = true }

  @Before fun startService() {
    val context: Context = RuntimeEnvironment.getApplication()
    ReaderLibrary::class.java.getDeclaredField("instance").apply { isAccessible = true }.set(null, null)
    val file = File(context.noBackupFilesDir, "private-reader/library.json")
    file.parentFile!!.mkdirs()
    file.writeText("""{"books":[{"id":"fixture","title":"Fixture","kind":"audio","available":true,
      "position":{"offsetMs":12345,"trackId":"chapter-1"},"bookmarks":[],
      "tracks":[{"id":"chapter-1","title":"One","relativePath":"one.mp3"},{"id":"chapter-2","title":"Two","relativePath":"two.mp3"}]}],
      "volume":{"enabled":false,"key":"up","gesture":"longPress"},"playbackRates":{"audio":1.75,"tts":3.5}}""")
    controller = Robolectric.buildService(PrivateReaderService::class.java).create()
    service = controller.get()
    player = field("player").get(service) as ExoPlayer
  }

  @After fun closeService() {
    field("bookId").set(service, null)
    controller.destroy()
    ReaderLibrary::class.java.getDeclaredField("instance").apply { isAccessible = true }.set(null, null)
  }

  @Test fun restoresSavedRateAndChangesPitchPreservingSpeedWithoutSeekingOrRecreatingQueue() {
    assertEquals(1.75f, player.playbackParameters.speed)
    player.setMediaItems(listOf(
      MediaItem.Builder().setMediaId("chapter-1").setUri("content://fixture/1").build(),
      MediaItem.Builder().setMediaId("chapter-2").setUri("content://fixture/2").build()
    ), 0, 12345)
    field("bookId").set(service, "fixture")
    val original = player.currentMediaItem
    for (rate in listOf(0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0)) {
      val state = service.setPlaybackRate(rate)
      assertEquals(rate, state.getDouble("playbackRate"), 0.0)
      assertTrue(state.isNull("error"))
      assertEquals(1f, player.playbackParameters.pitch)
      assertEquals(12345L, player.currentPosition)
      assertEquals(original, player.currentMediaItem)
      assertFalse(player.playWhenReady)
    }
    service.pause()
    assertEquals(4f, player.playbackParameters.speed)
    player.seekTo(1, 7000)
    assertEquals(4f, player.playbackParameters.speed)
    assertEquals(1f, player.playbackParameters.pitch)
    assertEquals(7000L, player.currentPosition)
    assertEquals(4f, ReaderLibrary.get(service).playbackRate("audio"))
    assertEquals(3.5f, ReaderLibrary.get(service).playbackRate("tts"))
  }

  @Test fun serviceRecreationRestoresSavedAudioAndKeepsSeparateNarrationPreference() {
    field("bookId").set(service, "fixture")
    service.setPlaybackRate(4)
    field("bookId").set(service, null)
    controller.destroy()
    ReaderLibrary::class.java.getDeclaredField("instance").apply { isAccessible = true }.set(null, null)
    controller = Robolectric.buildService(PrivateReaderService::class.java).create()
    service = controller.get()
    player = field("player").get(service) as ExoPlayer
    assertEquals(4.0, service.state().getDouble("playbackRate"), 0.0)
    assertEquals(1f, player.playbackParameters.pitch)
    assertEquals(3.5f, ReaderLibrary.get(service).playbackRate("tts"))
    assertFalse(player.playWhenReady)
  }

  @Test fun invalidRatesFailBeforeBookLookupOrAnyPlayerStateChange() {
    val before = service.state().toString()
    for (rate in listOf(null, "2", true, Double.NaN, Double.NEGATIVE_INFINITY, 0.49, 4.01)) {
      val failure = assertThrows(IllegalArgumentException::class.java) { service.setPlaybackRate(rate) }
      assertTrue(failure.message!!.startsWith("INVALID_PLAYBACK_RATE"))
    }
    for (method in listOf<(JSONObject) -> JSONObject>(service::play, service::startNarration)) {
      assertThrows(IllegalArgumentException::class.java) { method(JSONObject().put("playbackRate", 5)) }
    }
    assertEquals(before, service.state().toString())
    assertThrows(IllegalArgumentException::class.java) { service.setPlaybackRate(4) }
  }

  @Test fun preparingNarrationUsesItsOwnRateWithoutChangingVoiceOrQueue() {
    val narrator = ReaderNarrator(service, { _, _, _, _ -> }, {})
    field("narrator").set(service, narrator)
    field("ttsLoading").set(service, true)
    field("bookId").set(service, "fixture")
    val state = service.setPlaybackRate(4)
    assertEquals("tts", state.getString("mode"))
    assertEquals(4.0, state.getDouble("playbackRate"), 0.0)
    assertSame(narrator, field("narrator").get(service))
    assertEquals("", narrator.voiceId)
    assertEquals(1f, player.playbackParameters.pitch)
    assertEquals(1.75f, ReaderLibrary.get(service).playbackRate("audio"))
    assertEquals(4f, ReaderLibrary.get(service).playbackRate("tts"))
  }
}
