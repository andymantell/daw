// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.daw.remote.core.Layout
import org.daw.remote.core.Overview
import org.daw.remote.core.Status

/**
 * The whole song at a glance: a lane per track with its clips, bar
 * lines, markers, the loop and the playhead. Tap or drag to move the
 * playhead (and so where recording starts), snapping to bars and
 * markers. [onSeek] gets each position while dragging and `settled`
 * true on a tap or when the drag ends.
 */
@Composable
fun OverviewView(
    layout: Layout,
    status: Status?,
    modifier: Modifier = Modifier,
    onSeek: (frames: Long, settled: Boolean) -> Unit,
) {
    val overview = remember(layout) { Overview(layout) }
    val measurer = rememberTextMeasurer()
    // Where a finger is, drawn ahead of the desktop's answer.
    var dragging by remember { mutableStateOf<Long?>(null) }
    val laneGap = 2f

    Canvas(
        modifier = modifier
            .pointerInput(overview) {
                detectTapGestures { pos ->
                    val f = overview.snap(pos.x / size.width)
                    dragging = null
                    onSeek(f, true)
                }
            }
            .pointerInput(overview) {
                detectDragGestures(
                    onDragStart = { pos ->
                        dragging = overview.snap(pos.x / size.width)
                    },
                    onDrag = { change, _ ->
                        val f = overview.snap(change.position.x / size.width)
                        if (f != dragging) {
                            dragging = f
                            onSeek(f, false)
                        }
                    },
                    onDragEnd = {
                        dragging?.let { onSeek(it, true) }
                        dragging = null
                    },
                    onDragCancel = { dragging = null },
                )
            },
    ) {
        val w = size.width
        val h = size.height
        val rulerH = 18.dp.toPx()
        drawRect(Color(0xFF0C1014), size = size)

        // Loop region.
        if (layout.looping && layout.loopBars.size == 2) {
            val a = overview.fraction(overview.barStart(layout.loopBars[0])) * w
            val b = overview.fraction(overview.barStart(layout.loopBars[1])) * w
            drawRect(Color(0x338BB8FF), topLeft = Offset(a, 0f), size = Size(b - a, h))
        }

        // Bar lines, heavier every four bars.
        val bars = overview.bars
        val minPx = 6f
        val barPx = w / bars.toFloat()
        val every = if (barPx < minPx) (minPx / barPx).toInt().coerceAtLeast(1) else 1
        var bar = 1L
        while (bar <= bars) {
            val x = overview.fraction(overview.barStart(bar)) * w
            val strong = (bar - 1) % 4 == 0L
            drawLine(
                if (strong) Color(0xFF3A4654) else Color(0xFF222B35),
                Offset(x, rulerH),
                Offset(x, h),
                strokeWidth = 1f,
            )
            if (strong && barPx * 4 > 28f) {
                drawText(measurer, bar.toString(), Offset(x + 3f, 1f), TextStyle(color = Color(0xFF8A96A3), fontSize = 10.sp))
            }
            bar += every
        }

        // Lanes with clips.
        val lanes = layout.tracks.size.coerceAtLeast(1)
        val laneH = (h - rulerH - laneGap * (lanes + 1)) / lanes
        layout.tracks.forEachIndexed { i, t ->
            val top = rulerH + laneGap + i * (laneH + laneGap)
            drawRect(Color(0xFF151B22), topLeft = Offset(0f, top), size = Size(w, laneH))
            for (c in t.clips) {
                val x0 = overview.fraction(c.start) * w
                val x1 = overview.fraction(c.start + c.len) * w
                val color = when {
                    c.muted -> Color(0xFF2E3A33)
                    c.take != null -> TakeGreen
                    else -> FileBlue
                }
                drawRect(color, topLeft = Offset(x0, top + 1f), size = Size((x1 - x0).coerceAtLeast(2f), laneH - 2f))
            }
            if (t.armed) {
                drawRect(RecordRed, topLeft = Offset(0f, top), size = Size(4f, laneH))
            }
        }

        // Markers.
        for (m in layout.markers) {
            val x = overview.fraction(overview.barStart(m.bar)) * w
            drawLine(Color(0xFFE2B33C), Offset(x, 0f), Offset(x, h), strokeWidth = 2f)
            drawText(measurer, m.name, Offset(x + 4f, rulerH + 2f), TextStyle(color = Color(0xFFE2B33C), fontSize = 11.sp))
        }

        // Playhead: the finger's while dragging, else the desktop's.
        val at = dragging ?: status?.position
        if (at != null) {
            val x = overview.fraction(at) * w
            val color = if (status != null && status.recording && status.playing) RecordRed else Color.White
            drawLine(color, Offset(x, 0f), Offset(x, h), strokeWidth = 3f)
            drawCircle(color, radius = 7f, center = Offset(x, rulerH / 2f))
        }
        drawRect(Color(0xFF2A3440), size = size, style = Stroke(width = 1f))
    }
}
