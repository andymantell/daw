// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonClassDiscriminator
import kotlinx.serialization.encodeToString

/**
 * The messages between the phone and the desktop: one JSON object per
 * line, with a `type` field. The desktop's definitions are in
 * `crates/remote/src/protocol.rs`; `tests/fixtures` there holds examples
 * the tests here read, so the two can't drift apart.
 */
const val VERSION = 1

@OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
@Serializable
@JsonClassDiscriminator("type")
sealed interface FromPhone

@Serializable
@SerialName("hello")
data class Hello(
    val version: Int = VERSION,
    val name: String,
    val code: String? = null,
    val token: String? = null,
) : FromPhone

@Serializable @SerialName("ping") data object Ping : FromPhone

@Serializable @SerialName("play") data object Play : FromPhone

@Serializable @SerialName("stop") data object Stop : FromPhone

@Serializable @SerialName("to_start") data object ToStart : FromPhone

@Serializable @SerialName("record") data class Record(val on: Boolean) : FromPhone

@Serializable @SerialName("seek") data class Seek(val frames: Long) : FromPhone

@Serializable @SerialName("seek_bar") data class SeekBar(val bar: Long) : FromPhone

@Serializable @SerialName("arm") data class Arm(val track: Int, val on: Boolean) : FromPhone

@Serializable @SerialName("count_in") data class CountIn(val on: Boolean) : FromPhone

@Serializable @SerialName("metronome") data class Metronome(val on: Boolean) : FromPhone

@Serializable @SerialName("loop") data class Loop(val on: Boolean) : FromPhone

@Serializable @SerialName("undo_last_take") data object UndoLastTake : FromPhone

@OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
@Serializable
@JsonClassDiscriminator("type")
sealed interface ToPhone

@Serializable
@SerialName("welcome")
data class Welcome(val version: Int, val token: String, val desktop: String) : ToPhone

@Serializable @SerialName("denied") data class Denied(val reason: String) : ToPhone

@Serializable
@SerialName("layout")
data class Layout(
    val project: String,
    @SerialName("sample_rate") val sampleRate: Int,
    @SerialName("tempo_bpm") val tempoBpm: Double,
    @SerialName("beats_per_bar") val beatsPerBar: Int,
    @SerialName("bar_frames") val barFrames: Double,
    @SerialName("end_frames") val endFrames: Long,
    @SerialName("count_in") val countIn: Boolean,
    val metronome: Boolean,
    val looping: Boolean,
    @SerialName("loop_bars") val loopBars: List<Long>,
    @SerialName("may_record") val mayRecord: Boolean,
    val tracks: List<Track>,
    val markers: List<Marker>,
) : ToPhone

@Serializable
@SerialName("status")
data class Status(
    val playing: Boolean,
    @SerialName("counting_in") val countingIn: Boolean,
    val recording: Boolean,
    val position: Long,
    val bar: Long,
    val beat: Long,
    val seconds: Double,
    /** Peak per track, left and right. */
    val levels: List<List<Float>>,
    val master: List<Float>,
    @SerialName("can_undo_take") val canUndoTake: Boolean,
) : ToPhone

@Serializable
data class Track(
    val name: String,
    val armed: Boolean,
    @SerialName("can_arm") val canArm: Boolean,
    val clips: List<Clip>,
)

@Serializable
data class Clip(
    val start: Long,
    val len: Long,
    val take: Int? = null,
    val muted: Boolean = false,
)

@Serializable
data class Marker(val bar: Long, val name: String)

object Protocol {
    val json: Json = Json {
        ignoreUnknownKeys = true
        encodeDefaults = true
        explicitNulls = false
    }

    /** One message as a line, newline included. */
    fun encode(msg: FromPhone): String = json.encodeToString(msg) + "\n"

    /** A message from a line, or null if it isn't one we know. */
    fun decode(line: String): ToPhone? =
        try {
            json.decodeFromString<ToPhone>(line.trimEnd('\r', '\n'))
        } catch (e: kotlinx.serialization.SerializationException) {
            null
        } catch (e: IllegalArgumentException) {
            null
        }

    /** A phone-side message from a line; for the tests and the fixtures. */
    fun decodeFromPhone(line: String): FromPhone? =
        try {
            json.decodeFromString<FromPhone>(line.trimEnd('\r', '\n'))
        } catch (e: kotlinx.serialization.SerializationException) {
            null
        } catch (e: IllegalArgumentException) {
            null
        }
}

/** The text in the desktop's QR image, taken apart. */
data class Pairing(val host: String, val port: Int, val code: String) {
    companion object {
        const val SCHEME = "daw-remote"

        fun parse(text: String): Pairing? {
            val rest = text.trim().removePrefix("$SCHEME://")
            if (rest == text.trim()) return null
            val slash = rest.indexOf('/')
            if (slash < 0) return null
            val addr = rest.substring(0, slash)
            val code = rest.substring(slash + 1)
            if (code.isEmpty()) return null
            val host: String
            val port: String
            if (addr.startsWith("[")) {
                val end = addr.indexOf("]:")
                if (end < 0) return null
                host = addr.substring(1, end)
                port = addr.substring(end + 2)
            } else {
                val colon = addr.lastIndexOf(':')
                if (colon < 0) return null
                host = addr.substring(0, colon)
                port = addr.substring(colon + 1)
            }
            if (host.isEmpty()) return null
            val p = port.toIntOrNull() ?: return null
            if (p !in 1..65535) return null
            return Pairing(host, p, code)
        }
    }
}
