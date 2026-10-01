package com.grafium.app

import android.accessibilityservice.AccessibilityService
import android.app.KeyguardManager
import android.content.Context
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.view.KeyEvent
import android.view.accessibility.AccessibilityEvent
import org.json.JSONObject

/** Opt-in physical-key filtering only. Never reads windows or observes system volume. */
class ReaderVolumeService : AccessibilityService() {
  private val handler = Handler(Looper.getMainLooper())
  private val gesture = ReaderVolumeGesture()
  private val longPress = Runnable {
    val settings = try { ReaderLibrary.get(this).volume() } catch (_: Exception) { return@Runnable }
    val key = if (settings.optString("key") == "down") KeyEvent.KEYCODE_VOLUME_DOWN else KeyEvent.KEYCODE_VOLUME_UP
    if (!gesture.hold(settings.optBoolean("enabled") && key == gesture.heldKey &&
        PrivateReaderService.instance?.isActivelyPlaying() == true)) return@Runnable
    try {
      PrivateReaderService.instance!!.bookmark()
      gesture.saved()
      diagnostics.put("lastBookmarkAt", System.currentTimeMillis()).remove("error")
    } catch (_: Exception) {
      diagnostics.put("error", "BOOKMARK_SAVE_FAILED")
    }
    PrivateReaderBridge.publishCapabilities(this)
  }

  override fun onServiceConnected() {
    connected = true
    PrivateReaderBridge.publishCapabilities(this)
  }
  override fun onAccessibilityEvent(event: AccessibilityEvent?) = Unit
  override fun onInterrupt() { reset() }

  override fun onKeyEvent(event: KeyEvent): Boolean {
    if (event.keyCode != KeyEvent.KEYCODE_VOLUME_UP && event.keyCode != KeyEvent.KEYCODE_VOLUME_DOWN) return false
    val power = getSystemService(Context.POWER_SERVICE) as PowerManager
    val keyguard = getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager
    diagnostics.put("lastKeyEvent", JSONObject().put("key", if (event.keyCode == KeyEvent.KEYCODE_VOLUME_UP) "up" else "down")
      .put("action", event.action).put("repeat", event.repeatCount).put("at", System.currentTimeMillis())
      .put("locked", keyguard.isKeyguardLocked).put("screenOn", power.isInteractive))
    val settings = try { ReaderLibrary.get(this).volume() } catch (_: Exception) { return false }
    val configuredKey = if (settings.optString("key") == "down") KeyEvent.KEYCODE_VOLUME_DOWN else KeyEvent.KEYCODE_VOLUME_UP
    val active = settings.optBoolean("enabled") && PrivateReaderService.instance?.isActivelyPlaying() == true
    // Always finish a consumed down/up pair, even if playback paused during the press.
    if (event.action == KeyEvent.ACTION_UP && gesture.heldKey == event.keyCode) {
      handler.removeCallbacks(longPress)
      if (gesture.up(event.keyCode, event.isCanceled) == ReaderVolumeGesture.Release.ADJUST_VOLUME) {
        (getSystemService(Context.AUDIO_SERVICE) as AudioManager).adjustStreamVolume(
          AudioManager.STREAM_MUSIC,
          if (event.keyCode == KeyEvent.KEYCODE_VOLUME_UP) AudioManager.ADJUST_RAISE else AudioManager.ADJUST_LOWER,
          AudioManager.FLAG_SHOW_UI)
      }
      reset()
      PrivateReaderBridge.publishCapabilities(this)
      return true
    }
    if (event.action == KeyEvent.ACTION_DOWN) {
      val wasHeld = gesture.heldKey != null
      val consumed = gesture.down(event.keyCode, event.repeatCount, active && event.keyCode == configuredKey)
      if (consumed && !wasHeld) {
        handler.postDelayed(longPress, 700)
      }
      return consumed
    }
    return false
  }

  private fun reset() {
    handler.removeCallbacks(longPress)
    gesture.reset()
  }
  override fun onDestroy() {
    reset()
    connected = false
    super.onDestroy()
  }
  companion object {
    @Volatile var connected = false
      private set
    val diagnostics = JSONObject()
  }
}
