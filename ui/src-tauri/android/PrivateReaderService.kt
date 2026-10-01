package com.grafium.app

import android.app.PendingIntent
import android.content.Intent
import android.os.Handler
import android.os.Looper
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import org.json.JSONObject

@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
class PrivateReaderService : MediaSessionService() {
  private lateinit var player: ExoPlayer
  private lateinit var session: MediaSession
  private lateinit var library: ReaderLibrary
  private val handler = Handler(Looper.getMainLooper())
  private var bookId: String? = null
  private var error: String? = null
  private var replacing = false
  private var lastPosition = 0L
  private var lastTrack: String? = null
  private val resources = java.util.concurrent.ConcurrentHashMap<String, Pair<String, String>>()
  private val speechResources = java.util.concurrent.ConcurrentHashMap<String, java.io.File>()
  private var narrator: ReaderNarrator? = null
  private var ttsOrdinal = 0
  private var ttsLoading = false
  private var ttsResumeOffset = 0L
  private var narrationSourceTrackId = ""

  private val checkpointTask = object : Runnable {
    override fun run() {
      persist()
      publish()
      handler.postDelayed(this, 3000)
    }
  }

  override fun onCreate() {
    super.onCreate()
    library = ReaderLibrary.get(this)
    val sources = DataSource.Factory {
      val source = DefaultDataSource.Factory(this).createDataSource()
      object : DataSource by source {
        override fun open(dataSpec: DataSpec): Long {
          if (dataSpec.uri.scheme == "file") {
            val file = speechResources[dataSpec.uri.toString()] ?: throw java.io.IOException("RESOURCE_NOT_REGISTERED")
            if (!file.isFile || file.length() > 16L * 1024 * 1024) throw java.io.IOException("SPEECH_AUDIO_MISSING")
          } else {
            val registration = resources[dataSpec.uri.toString()] ?: throw java.io.IOException("RESOURCE_NOT_REGISTERED")
            try { library.verifyResource(registration.first, registration.second) }
            catch (error: Exception) { throw java.io.IOException("SOURCE_MISSING_OR_REPLACED", error) }
          }
          return source.open(dataSpec)
        }
      }
    }
    player = ExoPlayer.Builder(this).setMediaSourceFactory(DefaultMediaSourceFactory(sources)).build().apply {
      setAudioAttributes(AudioAttributes.Builder().setUsage(C.USAGE_MEDIA)
        .setContentType(C.AUDIO_CONTENT_TYPE_SPEECH).build(), true)
      setHandleAudioBecomingNoisy(true)
      setWakeMode(C.WAKE_MODE_LOCAL)
      addListener(object : Player.Listener {
        override fun onIsPlayingChanged(isPlaying: Boolean) { persist(); publish() }
        override fun onPlaybackStateChanged(playbackState: Int) { persist(); publish() }
        override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
          if (replacing) return
          if (narrator != null) { persist(); publish(); return }
          val id = bookId ?: return
          oldPosition.mediaItem?.mediaId?.let { oldTrack ->
            try { library.checkpoint(id, oldTrack, oldPosition.positionMs.coerceAtLeast(0)) }
            catch (_: Exception) { error = "PRIVATE_STATE_WRITE_FAILED" }
          }
          persist()
          publish()
        }
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
          if (!replacing && narrator != null && mediaItem != null) {
            ttsOrdinal = mediaItem.mediaId.removePrefix("tts-").toInt()
            narrator?.advanced(ttsOrdinal)
          }
          persist()
          publish()
        }
        override fun onPlayerError(playbackError: PlaybackException) {
          error = "PLAYBACK_SOURCE_UNAVAILABLE: check the local file and persisted read grant, then rescan or relink"
          persist()
          publish()
        }
      })
    }
    val launch = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java),
      PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    session = MediaSession.Builder(this, player).setSessionActivity(launch)
      .setCallback(object : MediaSession.Callback {
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
          // Notification/headphone clients may control transport, but cannot inject URLs or replace the native queue.
          val commands = MediaSession.ConnectionResult.DEFAULT_PLAYER_COMMANDS.buildUpon()
            .remove(Player.COMMAND_SET_MEDIA_ITEM).remove(Player.COMMAND_CHANGE_MEDIA_ITEMS).build()
          return MediaSession.ConnectionResult.AcceptedResultBuilder(session).setAvailablePlayerCommands(commands).build()
        }
      }).build()
    instance = this
    handler.post(checkpointTask)
  }

  override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession = session

  fun play(args: JSONObject): JSONObject {
    val id = args.getString("bookId")
    val book = library.book(id)
    require(book.getString("kind") == "audio") { "OFFLINE_VOICE_REQUIRED: EPUB narration needs an installed offline engine" }
    require(book.optBoolean("available")) { "SOURCE_MISSING: rescan or relink this book" }
    val tracks = book.getJSONArray("tracks")
    require(tracks.length() > 0) { "NO_REGISTERED_TRACKS" }
    val saved = book.getJSONObject("position")
    val chosen = args.optString("trackId").ifEmpty {
      saved.optString("trackId").ifEmpty { tracks.getJSONObject(0).getString("id") }
    }
    val index = (0 until tracks.length()).find { tracks.getJSONObject(it).getString("id") == chosen }
      ?: throw IllegalArgumentException("SAVED_TRACK_MISSING_OR_REPLACED: choose a track explicitly")
    val offset = if (args.has("offsetMs")) args.getLong("offsetMs")
      else if (chosen == saved.optString("trackId")) saved.optLong("offsetMs") else 0L
    require(offset >= 0) { "INVALID_POSITION" }
    val queue = (0 until tracks.length()).map {
      val track = tracks.getJSONObject(it)
      val uri = library.resource(id, track.getString("id"))
      MediaItem.Builder().setMediaId(track.getString("id"))
        .setUri(uri)
        .setMediaMetadata(MediaMetadata.Builder().setTitle(track.getString("title"))
          .setAlbumTitle(book.getString("title")).setIsPlayable(true).build()).build()
    }
    persist()
    narrator?.close()
    narrator = null
    ttsLoading = false
    speechResources.clear()
    replacing = true
    try {
      player.pause()
      bookId = id
      error = null
      resources.clear()
      queue.forEach { resources[it.localConfiguration!!.uri.toString()] = id to it.mediaId }
      player.setMediaItems(queue, index, offset)
      player.prepare()
      player.play()
    } finally { replacing = false }
    persist()
    publish()
    return state()
  }

  fun pause(): JSONObject { player.pause(); persist(); publish(); return state() }
  fun resume(): JSONObject {
    require(bookId != null) { "NO_ACTIVE_BOOK: select a book to resume saved progress" }
    if (player.playbackState == Player.STATE_IDLE) player.prepare()
    player.play()
    publish()
    return state()
  }
  fun stop(): JSONObject {
    persist()
    if (ttsLoading) stopForeground(STOP_FOREGROUND_REMOVE)
    replacing = true
    try {
      player.pause()
      player.stop()
      player.clearMediaItems()
      bookId = null
      narrator?.close()
      narrator = null
      ttsLoading = false
      speechResources.clear()
      lastTrack = null
      resources.clear()
    } finally { replacing = false }
    publish()
    return state()
  }
  fun seek(offsetMs: Long): JSONObject {
    require(bookId != null && offsetMs >= 0) { "INVALID_POSITION" }
    player.seekTo(offsetMs)
    persist()
    publish()
    return state()
  }
  fun bookmark(note: String = ""): JSONObject {
    val id = bookId ?: throw IllegalStateException("NO_ACTIVE_BOOK")
    if (narrator != null) {
      require(!ttsLoading) { "NARRATION_PREPARING" }
      val mark = library.bookmarkAt(id, narrationPosition(), note)
      publish()
      return mark
    }
    val track = player.currentMediaItem?.mediaId ?: throw IllegalStateException("NO_ACTIVE_TRACK")
    val mark = library.bookmark(id, track, player.currentPosition.coerceAtLeast(0), note)
    publish()
    return mark
  }
  fun isActivelyPlaying(): Boolean = player.isPlaying
  fun activeBookId(): String? = bookId

  private fun persist() {
    if (replacing) return
    val id = bookId ?: return
    if (narrator != null) {
      if (ttsLoading) return
      try { library.narrationCheckpoint(id, narrationPosition()) }
      catch (_: Exception) { error = "PRIVATE_STATE_WRITE_FAILED"; if (player.isPlaying) player.pause() }
      return
    }
    val track = player.currentMediaItem?.mediaId ?: return
    val offset = player.currentPosition.coerceAtLeast(0)
    if (track == lastTrack && offset == lastPosition) return
    try {
      library.checkpoint(id, track, offset)
      lastTrack = track
      lastPosition = offset
    } catch (_: Exception) {
      error = "PRIVATE_STATE_WRITE_FAILED: playback paused to avoid losing progress"
      if (player.isPlaying) player.pause()
    }
  }

  fun state(): JSONObject = JSONObject().put("bookId", bookId ?: JSONObject.NULL)
    .put("trackId", player.currentMediaItem?.mediaId ?: JSONObject.NULL)
    .put("offsetMs", player.currentPosition.coerceAtLeast(0))
    .put("durationMs", player.duration.takeIf { it != C.TIME_UNSET } ?: 0)
    .put("playing", player.isPlaying).put("buffering", player.playbackState == Player.STATE_BUFFERING)
    .put("error", error ?: JSONObject.NULL).put("checkpointIntervalMs", 3000)
    .put("mode", if (narrator != null) "tts" else "audio")
    .put("ttsLoading", ttsLoading).put("ordinal", ttsOrdinal).put("segmentCount", narrator?.count ?: 0)
    .put("locator", if (narrator != null && !ttsLoading) narrationPosition().getJSONObject("locator") else JSONObject.NULL)

  private fun narrationPosition(): JSONObject {
    val ordinal = player.currentMediaItem?.mediaId?.removePrefix("tts-")?.toIntOrNull() ?: ttsOrdinal
    return JSONObject().put("locator", narrator!!.locator(ordinal))
      .put("ttsOrdinal", ordinal).put("sourceHash", narrator!!.sourceHash)
      .put("sourceTrackId", narrationSourceTrackId).put("voiceId", narrator!!.voiceId)
      .put("offsetMs", player.currentPosition.coerceAtLeast(0))
  }

  fun startNarration(args: JSONObject): JSONObject {
    val id = args.getString("bookId")
    val book = library.book(id)
    require(book.getString("kind") == "epub" && book.optBoolean("available")) { "EPUB_SOURCE_UNAVAILABLE" }
    require(!args.has("ordinal")) { "CANONICAL_NARRATION_START_REQUIRED: use fromBeginning rather than ordinal" }
    val fromBeginning = args.optBoolean("fromBeginning", false)
    val savedPosition = book.getJSONObject("position")
    require(fromBeginning || !savedPosition.has("sourceTrackId") ||
      savedPosition.getString("sourceTrackId") == book.getJSONArray("tracks").getJSONObject(0).getString("id")) {
      "NARRATION_SOURCE_CHANGED: choose Read from beginning explicitly"
    }
    stop()
    bookId = id
    error = null
    ttsLoading = true
    ttsOrdinal = 0
    narrationSourceTrackId = book.getJSONArray("tracks").getJSONObject(0).getString("id")
    ttsResumeOffset = if (fromBeginning) 0 else savedPosition.optLong("offsetMs", 0)
    player.playWhenReady = true
    val manager = getSystemService(android.app.NotificationManager::class.java)
    if (android.os.Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(android.app.NotificationChannel(
      "private_reader_preparing", "Private reader preparation", android.app.NotificationManager.IMPORTANCE_LOW))
    startForeground(1001, androidx.core.app.NotificationCompat.Builder(this, "private_reader_preparing")
      .setSmallIcon(android.R.drawable.ic_media_play).setContentTitle("Grafium private reader")
      .setContentText("Preparing offline narration").setOngoing(true).build())
    val pending = ReaderNarrator(this, { ordinal, file, _, first ->
      if (narrator == null) return@ReaderNarrator
      val uri = android.net.Uri.fromFile(file)
      speechResources[uri.toString()] = file
      val item = MediaItem.Builder().setMediaId("tts-$ordinal").setUri(uri)
        .setMediaMetadata(MediaMetadata.Builder().setTitle(book.getString("title"))
          .setSubtitle("Offline narration ${ordinal + 1}").setIsPlayable(true).build()).build()
      if (first) {
        replacing = true
        player.setMediaItem(item, ttsResumeOffset)
        replacing = false
        ttsLoading = false
        player.prepare()
      } else {
        player.addMediaItem(item)
        if (player.playbackState == Player.STATE_ENDED) { player.seekToNextMediaItem(); player.prepare() }
        val previous = player.currentMediaItemIndex
        if (previous > 0) player.removeMediaItems(0, previous)
        speechResources.keys.removeAll { key -> key != uri.toString() &&
          key != player.currentMediaItem?.localConfiguration?.uri.toString() }
      }
      persist()
      publish()
    }, { message ->
      error = message
      ttsLoading = false
      player.pause()
      stopForeground(STOP_FOREGROUND_REMOVE)
      publish()
    })
    narrator = pending
    try {
      ttsOrdinal = pending.start(id, savedPosition, fromBeginning, args.getString("_verifiedSourceHash"))
      if (savedPosition.optString("voiceId") != pending.voiceId) ttsResumeOffset = 0
    }
    catch (failure: Exception) { stop(); stopForeground(STOP_FOREGROUND_REMOVE); throw failure }
    publish()
    return state()
  }

  private fun publish() { PrivateReaderBridge.publish(state()) }
  override fun onTaskRemoved(rootIntent: Intent?) {
    persist()
    if (!player.playWhenReady && !ttsLoading) stopSelf()
  }
  override fun onDestroy() {
    persist()
    handler.removeCallbacksAndMessages(null)
    instance = null
    narrator?.close()
    session.release()
    player.release()
    super.onDestroy()
  }
  companion object {
    @Volatile var instance: PrivateReaderService? = null
      private set
  }
}
