package com.grafium.app

import org.junit.Assert.*
import org.junit.Test

class ReaderVolumeGestureTest {
  @Test fun disabledIdleAndUnselectedKeysPassThrough() {
    val gesture = ReaderVolumeGesture()
    assertFalse(gesture.down(24, 0, false))
    assertFalse(gesture.hold(false))
    assertEquals(ReaderVolumeGesture.Release.PASS, gesture.up(24, false))
    assertNull(gesture.heldKey)
  }
  @Test fun shortPressAdjustsVolumeOnlyOnRelease() {
    val gesture = ReaderVolumeGesture()
    assertTrue(gesture.down(24, 0, true))
    assertEquals(ReaderVolumeGesture.Release.ADJUST_VOLUME, gesture.up(24, false))
    assertFalse(gesture.hold(true))
  }
  @Test fun longPressBookmarksOnceAndDoesNotAdjustVolume() {
    val gesture = ReaderVolumeGesture()
    assertTrue(gesture.down(25, 0, true))
    assertTrue(gesture.hold(true))
    gesture.saved()
    assertTrue(gesture.down(25, 3, true))
    assertFalse(gesture.hold(true))
    assertEquals(ReaderVolumeGesture.Release.CONSUME, gesture.up(25, false))
    assertTrue(gesture.down(25, 0, true))
    assertTrue(gesture.hold(true))
  }
  @Test fun disablingDuringHoldPreventsBookmarkButCompletesPair() {
    val gesture = ReaderVolumeGesture()
    assertTrue(gesture.down(24, 0, true))
    assertFalse(gesture.hold(false))
    assertTrue(gesture.down(24, 1, false))
    assertEquals(ReaderVolumeGesture.Release.ADJUST_VOLUME, gesture.up(24, false))
  }
  @Test fun cancelledAndUnpairedRepetitionsDoNotAct() {
    val gesture = ReaderVolumeGesture()
    assertFalse(gesture.down(24, 1, true))
    assertTrue(gesture.down(24, 0, true))
    assertFalse(gesture.down(25, 0, true))
    assertEquals(ReaderVolumeGesture.Release.PASS, gesture.up(25, false))
    assertEquals(ReaderVolumeGesture.Release.CONSUME, gesture.up(24, true))
    assertFalse(gesture.hold(true))
  }
}
