// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.core

import org.junit.Assert.assertEquals
import org.junit.Test

class OverviewTest {
    private val layout = Layout(
        project = "p", sampleRate = 48_000, tempoBpm = 120.0, beatsPerBar = 4,
        barFrames = 96_000.0, endFrames = 960_000, countIn = false, metronome = false,
        looping = false, loopBars = listOf(1, 2), mayRecord = true, tracks = emptyList(),
        markers = listOf(Marker(bar = 3, name = "Chorus")),
    )
    private val ov = Overview(layout)

    @Test
    fun framesAndFractionsGoThereAndBack() {
        assertEquals(960_000L + 96_000L, ov.span)
        assertEquals(0f, ov.fraction(0), 0f)
        assertEquals(1f, ov.fraction(ov.span * 2), 0f)
        assertEquals(ov.span / 2, ov.frames(0.5f))
        assertEquals(192_000L, ov.barStart(3))
        assertEquals(3L, ov.barAt(200_000))
        assertEquals(12L, ov.bars)
    }

    @Test
    fun tapsSnapToBarsAndMarkers() {
        // Just past bar 5's line snaps back to it.
        assertEquals(ov.barStart(5), ov.snap(ov.fraction(ov.barStart(5) + 10_000)))
        // Nearer bar 6 than 5 goes forward.
        assertEquals(ov.barStart(6), ov.snap(ov.fraction(ov.barStart(5) + 60_000)))
        // Within the window of the marker at bar 3, the marker wins.
        assertEquals(ov.barStart(3), ov.snap(ov.fraction(ov.barStart(3) + 15_000)))
        // Before the start stays at the start.
        assertEquals(0L, ov.snap(-1f))
    }
}
