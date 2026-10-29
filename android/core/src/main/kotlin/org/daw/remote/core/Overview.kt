// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.core

import kotlin.math.max
import kotlin.math.roundToLong

/**
 * The timeline overview's sums, kept out of the drawing code so they
 * can be tested: frames to a fraction of the width and back, and
 * snapping a tap to the nearest bar or marker.
 */
class Overview(private val layout: Layout) {
    /** The frames shown: the song, with a bar to spare after it. */
    val span: Long = max(layout.endFrames + layout.barFrames.roundToLong(), layout.barFrames.roundToLong() * 4)

    /** Where a frame falls across the width, from 0 to 1. */
    fun fraction(frames: Long): Float = (frames.toDouble() / span).toFloat().coerceIn(0f, 1f)

    /** The frame at a fraction of the width. */
    fun frames(fraction: Float): Long = (fraction.toDouble().coerceIn(0.0, 1.0) * span).roundToLong()

    /** The first frame of a bar (bars count from 1). */
    fun barStart(bar: Long): Long = ((bar - 1).coerceAtLeast(0) * layout.barFrames).roundToLong()

    /** The bar a frame is in (from 1). */
    fun barAt(frames: Long): Long = (frames / layout.barFrames).toLong() + 1

    /** The bars shown, so each gets a line. */
    val bars: Long get() = (span / layout.barFrames).toLong() + 1

    /**
     * Where a tap at [fraction] lands: the nearest bar line, or a
     * marker's bar when one is within [snapFraction] of the width.
     */
    fun snap(fraction: Float, snapFraction: Float = 0.02f): Long {
        val at = frames(fraction)
        val nearestBar = ((at / layout.barFrames).roundToLong() + 1).coerceAtLeast(1)
        var best = barStart(nearestBar)
        val window = (snapFraction * span).roundToLong()
        for (m in layout.markers) {
            val mf = barStart(m.bar)
            if (kotlin.math.abs(mf - at) <= window && kotlin.math.abs(mf - at) < kotlin.math.abs(best - at)) {
                best = mf
            }
        }
        return best
    }
}
