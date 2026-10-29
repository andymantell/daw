// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.core

import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A stand-in desktop on the loopback address, one phone at a time. */
private class FakeDesktop {
    val server = ServerSocket(0)
    val port get() = server.localPort
    val received = LinkedBlockingQueue<String>()
    @Volatile var current: Socket? = null

    /** Waits for a phone, reads its hello, answers, then serves until hung up. */
    fun serveOnce(reply: ToPhoneLine, then: List<String> = emptyList()): Thread {
        val t = Thread {
            val s = server.accept()
            current = s
            s.use {
                val r = BufferedReader(InputStreamReader(s.getInputStream()))
                val w = OutputStreamWriter(s.getOutputStream())
                received.put(r.readLine())
                w.write(reply.line + "\n")
                then.forEach { w.write(it + "\n") }
                w.flush()
                if (reply.keepOpen) {
                    while (true) {
                        val l = try { r.readLine() ?: break } catch (e: java.io.IOException) { break }
                        received.put(l)
                    }
                }
            }
        }
        t.isDaemon = true
        t.start()
        return t
    }

    fun hangUp() = current?.close()

    fun close() = server.close()
}

private data class ToPhoneLine(val line: String, val keepOpen: Boolean)

private class Recorder : ConnectionListener {
    val states = LinkedBlockingQueue<ConnectionState>()
    val welcomes = LinkedBlockingQueue<Welcome>()
    val layouts = LinkedBlockingQueue<Layout>()
    val statuses = LinkedBlockingQueue<Status>()
    val denials = LinkedBlockingQueue<String>()
    override fun onState(state: ConnectionState) { states.put(state) }
    override fun onWelcome(welcome: Welcome) { welcomes.put(welcome) }
    override fun onLayout(layout: Layout) { layouts.put(layout) }
    override fun onStatus(status: Status) { statuses.put(status) }
    override fun onDenied(reason: String) { denials.put(reason) }
}

class ConnectionTest {
    private fun <T> take(q: LinkedBlockingQueue<T>): T {
        val v = q.poll(5, TimeUnit.SECONDS)
        assertNotNull("nothing arrived in time", v)
        return v!!
    }

    @Test
    fun pairsWithTheCodeThenComesBackWithTheToken() {
        val desk = FakeDesktop()
        val rec = Recorder()
        val welcome = """{"type":"welcome","version":1,"token":"TOK","desktop":"Desk"}"""
        val status = """{"type":"status","playing":false,"counting_in":false,"recording":false,"position":0,"bar":1,"beat":1,"seconds":0.0,"levels":[],"master":[0.0,0.0],"can_undo_take":false}"""
        desk.serveOnce(ToPhoneLine(welcome, keepOpen = true), listOf(status))
        val c = Connection("127.0.0.1", desk.port, "Phone", code = "CODE", token = null, listener = rec)
        c.start()

        val hello = Protocol.decodeFromPhone(take(desk.received)) as Hello
        assertEquals("CODE", hello.code)
        assertEquals(VERSION, hello.version)
        assertEquals("TOK", take(rec.welcomes).token)
        assertEquals(1L, take(rec.statuses).bar)
        assertTrue(rec.states.contains(ConnectionState.CONNECTED))

        c.send(Play)
        assertEquals(Play, Protocol.decodeFromPhone(take(desk.received)))

        // The desktop goes away; the phone tries again with its token.
        desk.hangUp()
        desk.serveOnce(ToPhoneLine(welcome, keepOpen = true))
        val hello2 = Protocol.decodeFromPhone(take(desk.received)) as Hello
        assertEquals("TOK", hello2.token)
        assertEquals(null, hello2.code)
        assertEquals("TOK", take(rec.welcomes).token)
        c.stop()
        desk.close()
    }

    @Test
    fun aDenialStopsTheTrying() {
        val desk = FakeDesktop()
        val rec = Recorder()
        desk.serveOnce(ToPhoneLine("""{"type":"denied","reason":"no"}""", keepOpen = false))
        val c = Connection("127.0.0.1", desk.port, "Phone", code = "BAD", token = null, listener = rec)
        c.start()
        assertEquals("no", take(rec.denials))
        assertEquals(ConnectionState.STOPPED, take(rec.states.also { q ->
            // Skip CONNECTING.
            while (q.peek() != null && q.peek() != ConnectionState.STOPPED) q.poll()
        }))
        desk.close()
    }
}
