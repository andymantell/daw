// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.daw.remote.Remote
import org.daw.remote.core.Arm
import org.daw.remote.core.ConnectionState
import org.daw.remote.core.CountIn
import org.daw.remote.core.FromPhone
import org.daw.remote.core.Loop
import org.daw.remote.core.Metronome
import org.daw.remote.core.Play
import org.daw.remote.core.Record
import org.daw.remote.core.Seek
import org.daw.remote.core.Stop
import org.daw.remote.core.ToStart
import org.daw.remote.core.UndoLastTake

/**
 * The remote itself: big transport buttons that work with sticks in
 * hand, the tracks with arm buttons and meters, and the overview to
 * move where recording starts. Sideways, the buttons sit beside the
 * overview; upright, above it.
 */
@Composable
fun RemoteScreen(state: Remote, send: (FromPhone) -> Unit, onForget: () -> Unit) {
    val haptic = LocalHapticFeedback.current
    val tap: (FromPhone) -> Unit = { msg ->
        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
        send(msg)
    }
    val connected = state.connection == ConnectionState.CONNECTED
    BoxWithConstraints(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .padding(12.dp),
    ) {
        val sideways = maxWidth > maxHeight
        Column(modifier = Modifier.fillMaxSize()) {
            Header(state, onForget)
            if (sideways) {
                Row(modifier = Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Column(modifier = Modifier.weight(0.42f).fillMaxHeight()) {
                        Transport(state, connected, tap, modifier = Modifier.fillMaxWidth())
                    }
                    Column(modifier = Modifier.weight(0.58f).fillMaxHeight()) {
                        OverviewAndTracks(state, connected, tap, send)
                    }
                }
            } else {
                Transport(state, connected, tap, modifier = Modifier.fillMaxWidth())
                Spacer(Modifier.height(8.dp))
                OverviewAndTracks(state, connected, tap, send)
            }
        }
    }
}

@Composable
private fun Header(state: Remote, onForget: () -> Unit) {
    val name = state.layout?.project ?: "…"
    val desk = state.desktopName ?: state.desktopAddress ?: ""
    val (dot, word) = when (state.connection) {
        ConnectionState.CONNECTED -> TakeGreen to "connected"
        ConnectionState.CONNECTING -> Color.Yellow to "connecting…"
        ConnectionState.RECONNECTING -> Color.Yellow to "looking for the desktop…"
        ConnectionState.STOPPED, null -> Color.Gray to "off"
    }
    Row(modifier = Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Box(
            modifier = Modifier
                .width(10.dp)
                .height(10.dp)
                .clip(RoundedCornerShape(5.dp))
                .background(dot),
        )
        Spacer(Modifier.width(8.dp))
        Text(
            text = "$name · $desk · $word",
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        TextButton(onClick = onForget) { Text("Pair again") }
    }
}

@Composable
private fun Transport(state: Remote, connected: Boolean, tap: (FromPhone) -> Unit, modifier: Modifier) {
    val status = state.status
    val layout = state.layout
    val mayRecord = layout?.mayRecord ?: false
    val playing = status?.playing ?: false
    val recording = status?.recording ?: false
    Column(modifier = modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        // Where we are, big enough to read from the kit.
        val where = when {
            status == null -> "—"
            status.countingIn -> "count-in"
            else -> "${status.bar}.${status.beat}"
        }
        Row(verticalAlignment = Alignment.Bottom) {
            Text(
                text = where,
                fontSize = 44.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
                color = if (recording && playing) RecordRed else MaterialTheme.colorScheme.onSurface,
            )
            Spacer(Modifier.width(12.dp))
            Text(
                text = status?.let { "%.1f s".format(it.seconds) } ?: "",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(bottom = 8.dp),
            )
        }
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            BigButton(
                label = if (recording) "⏺ Recording" else "⏺ Record",
                enabled = connected && mayRecord,
                color = if (recording) RecordRed else MaterialTheme.colorScheme.surfaceVariant,
                textColor = if (recording) Color.White else MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.weight(1f),
            ) { tap(Record(on = !recording)) }
            BigButton(
                label = if (playing) "⏹ Stop" else "▶ Play",
                enabled = connected,
                color = if (playing) MaterialTheme.colorScheme.surfaceVariant else MaterialTheme.colorScheme.primary,
                textColor = if (playing) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onPrimary,
                modifier = Modifier.weight(1f),
            ) { tap(if (playing) Stop else Play) }
        }
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            BigButton(
                label = "⏮ Start",
                enabled = connected,
                color = MaterialTheme.colorScheme.surfaceVariant,
                textColor = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.weight(1f),
                height = 64.dp,
            ) { tap(ToStart) }
            BigButton(
                label = "↶ Undo last take",
                enabled = connected && mayRecord && (status?.canUndoTake ?: false),
                color = MaterialTheme.colorScheme.surfaceVariant,
                textColor = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.weight(1f),
                height = 64.dp,
            ) { tap(UndoLastTake) }
        }
        Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val countIn = layout?.countIn ?: false
            val metronome = layout?.metronome ?: false
            val looping = layout?.looping ?: false
            FilterChip(selected = countIn, enabled = connected, onClick = { tap(CountIn(!countIn)) }, label = { Text("Count-in") })
            FilterChip(selected = metronome, enabled = connected, onClick = { tap(Metronome(!metronome)) }, label = { Text("Click") })
            FilterChip(
                selected = looping,
                enabled = connected,
                onClick = { tap(Loop(!looping)) },
                label = { Text(layout?.let { "Loop ${it.loopBars[0]}–${it.loopBars[1]}" } ?: "Loop") },
            )
        }
        if (layout != null && !mayRecord) {
            Text("You may only comment on this project, so recording is off.", color = MaterialTheme.colorScheme.error)
        }
    }
}

@Composable
private fun BigButton(
    label: String,
    enabled: Boolean,
    color: Color,
    textColor: Color,
    modifier: Modifier = Modifier,
    height: androidx.compose.ui.unit.Dp = 88.dp,
    onClick: () -> Unit,
) {
    Button(
        onClick = onClick,
        enabled = enabled,
        colors = ButtonDefaults.buttonColors(containerColor = color, contentColor = textColor),
        shape = RoundedCornerShape(14.dp),
        modifier = modifier.height(height),
    ) {
        Text(label, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

@Composable
private fun OverviewAndTracks(state: Remote, connected: Boolean, tap: (FromPhone) -> Unit, send: (FromPhone) -> Unit) {
    val layout = state.layout
    val status = state.status
    Column(modifier = Modifier.fillMaxSize()) {
        if (layout != null) {
            OverviewView(
                layout = layout,
                status = status,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(if (layout.tracks.size > 4) 160.dp else 120.dp),
                // Dragging sends many seeks; only the tap buzzes.
                onSeek = { frames, settled -> if (settled) tap(Seek(frames)) else send(Seek(frames)) },
            )
            Spacer(Modifier.height(8.dp))
            Tracks(layout, status, connected, tap)
        } else {
            Text(
                if (connected) "Waiting for the project…" else "Looking for the desktop. Is it on the same Wi-Fi, with the app open?",
                style = MaterialTheme.typography.bodyLarge,
            )
        }
    }
}

@Composable
private fun Tracks(layout: org.daw.remote.core.Layout, status: org.daw.remote.core.Status?, connected: Boolean, tap: (FromPhone) -> Unit) {
    // When a track last hit full scale, so the warning stays readable.
    var clippedAt by remember { mutableStateOf(mapOf<Int, Long>()) }
    val now = System.currentTimeMillis()
    status?.levels?.forEachIndexed { i, lr ->
        if (lr.any { it >= 0.999f } && clippedAt[i]?.let { now - it < 1500 } != true) {
            clippedAt = clippedAt + (i to now)
        }
    }
    LazyColumn(modifier = Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        itemsIndexed(layout.tracks) { i, t ->
            val level = status?.levels?.getOrNull(i) ?: listOf(0f, 0f)
            val clipped = clippedAt[i]?.let { now - it < 1500 } == true
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(10.dp))
                    .background(MaterialTheme.colorScheme.surface)
                    .padding(horizontal = 10.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Button(
                    onClick = { tap(Arm(track = i, on = !t.armed)) },
                    enabled = connected && t.canArm && layout.mayRecord,
                    colors = ButtonDefaults.buttonColors(
                        containerColor = if (t.armed) RecordRed else MaterialTheme.colorScheme.surfaceVariant,
                        contentColor = if (t.armed) Color.White else MaterialTheme.colorScheme.onSurface,
                    ),
                    shape = RoundedCornerShape(10.dp),
                    modifier = Modifier.height(52.dp).width(72.dp),
                ) {
                    Text("Arm", fontWeight = FontWeight.Bold)
                }
                Spacer(Modifier.width(10.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Row {
                        Text(t.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        if (clipped) Text("CLIP", color = RecordRed, fontWeight = FontWeight.Bold)
                        else if (!t.canArm) Text("no input", style = MaterialTheme.typography.labelSmall, color = Color.Gray)
                    }
                    Spacer(Modifier.height(4.dp))
                    Meter(level.getOrElse(0) { 0f })
                    Spacer(Modifier.height(2.dp))
                    Meter(level.getOrElse(1) { 0f })
                }
            }
        }
    }
}

/** A peak bar: green, amber near the top, red at full scale. */
@Composable
private fun Meter(level: Float) {
    val db = if (level <= 0f) -60f else (20f * kotlin.math.log10(level)).coerceIn(-60f, 0f)
    val fraction = (db + 60f) / 60f
    val color = when {
        level >= 0.999f -> RecordRed
        db > -6f -> Color(0xFFE2B33C)
        else -> TakeGreen
    }
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .height(8.dp)
            .clip(RoundedCornerShape(4.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant),
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth(fraction)
                .fillMaxHeight()
                .background(color),
        )
    }
}
