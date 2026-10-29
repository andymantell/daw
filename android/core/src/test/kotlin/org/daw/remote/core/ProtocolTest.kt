// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.core

import java.io.File
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ProtocolTest {
    private val fixtures = File(System.getProperty("fixtures") ?: "../../crates/remote/tests/fixtures")

    private fun fixture(name: String): String = File(fixtures, "$name.json").readText()

    private fun same(a: String, b: String) =
        assertEquals(Json.parseToJsonElement(a), Json.parseToJsonElement(b))

    @Test
    fun phoneMessagesMatchTheDesktopsFixtures() {
        val ours = mapOf(
            "hello" to Hello(name = "Andy's phone", code = "ABCD2345EFGH6789"),
            "hello_again" to Hello(name = "Andy's phone", token = "TOKEN"),
            "ping" to Ping,
            "play" to Play,
            "stop" to Stop,
            "to_start" to ToStart,
            "record" to Record(on = true),
            "seek" to Seek(frames = 96_000),
            "seek_bar" to SeekBar(bar = 17),
            "arm" to Arm(track = 1, on = true),
            "count_in" to CountIn(on = false),
            "metronome" to Metronome(on = true),
            "loop" to Loop(on = true),
            "undo_last_take" to UndoLastTake,
        )
        for ((name, msg) in ours) {
            val line = Protocol.encode(msg)
            assertTrue("$name ends in a newline", line.endsWith("\n"))
            same(fixture(name), line)
            assertEquals(name, msg, Protocol.decodeFromPhone(fixture(name)))
        }
        assertTrue(fixtures.listFiles()!!.count { it.extension == "json" } >= ours.size + 4)
    }

    @Test
    fun desktopMessagesAreReadFromTheFixtures() {
        val welcome = Protocol.decode(fixture("welcome")) as Welcome
        assertEquals("TOKEN", welcome.token)
        assertEquals("STUDIO-PC", welcome.desktop)
        assertEquals(VERSION, welcome.version)

        val denied = Protocol.decode(fixture("denied")) as Denied
        assertTrue(denied.reason.contains("scan"))

        val layout = Protocol.decode(fixture("layout")) as Layout
        assertEquals("Lesson 3", layout.project)
        assertEquals(48_000, layout.sampleRate)
        assertEquals(96_000.0, layout.barFrames, 0.0)
        assertEquals(listOf(3L, 5L), layout.loopBars)
        assertEquals(2, layout.tracks.size)
        assertEquals("Drums", layout.tracks[0].name)
        assertTrue(layout.tracks[0].armed && layout.tracks[0].canArm)
        assertEquals(3, layout.tracks[0].clips.size)
        assertNull(layout.tracks[0].clips[0].take)
        assertEquals(1, layout.tracks[0].clips[2].take)
        assertTrue(layout.tracks[0].clips[1].muted)
        assertEquals("Chorus", layout.markers[1].name)

        val status = Protocol.decode(fixture("status")) as Status
        assertTrue(status.playing && status.recording && !status.countingIn)
        assertEquals(240_000L, status.position)
        assertEquals(3L, status.bar)
        assertEquals(listOf(0.5f, 0.25f), status.levels[0])
        assertEquals(0.75f, status.master[0])
        assertTrue(status.canUndoTake)
    }

    @Test
    fun unknownFieldsAndMessagesAreTolerated() {
        assertNotNull(Protocol.decode("""{"type":"denied","reason":"x","later":1}"""))
        assertNull(Protocol.decode("""{"type":"dance"}"""))
        assertNull(Protocol.decode("not json"))
    }

    @Test
    fun pairingTextIsTakenApart() {
        assertEquals(
            Pairing("192.168.1.20", 7345, "ABCD2345EFGH6789"),
            Pairing.parse("daw-remote://192.168.1.20:7345/ABCD2345EFGH6789"),
        )
        assertEquals(Pairing("fe80::1", 7345, "CODE"), Pairing.parse(" daw-remote://[fe80::1]:7345/CODE\n"))
        assertNull(Pairing.parse("https://example.com/x"))
        assertNull(Pairing.parse("daw-remote://host:notaport/CODE"))
        assertNull(Pairing.parse("daw-remote://host:1/"))
    }
}
