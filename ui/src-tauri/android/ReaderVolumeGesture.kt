package com.grafium.app

/** Pure key-pair state machine; Android schedules the 700 ms hold timer. */
internal class ReaderVolumeGesture {
  enum class Release { PASS, CONSUME, ADJUST_VOLUME }
  var heldKey: Int? = null
    private set
  private var attempted = false
  private var saved = false
  fun down(key: Int, repeat: Int, eligible: Boolean): Boolean {
    if (heldKey != null) return heldKey == key
    if (!eligible || repeat != 0) return false
    heldKey = key
    return true
  }
  fun hold(eligible: Boolean): Boolean {
    if (heldKey == null || !eligible || attempted) return false
    attempted = true
    return true
  }
  fun saved() { saved = true }
  fun up(key: Int, cancelled: Boolean): Release {
    if (heldKey != key) return Release.PASS
    val result = if (saved || cancelled) Release.CONSUME else Release.ADJUST_VOLUME
    reset()
    return result
  }
  fun reset() { heldKey = null; attempted = false; saved = false }
}
