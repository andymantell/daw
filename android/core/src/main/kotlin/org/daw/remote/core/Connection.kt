// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.core

import java.io.BufferedReader
import java.io.IOException
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.io.Writer
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.min

/** What the connection tells the app. Called on the connection's thread. */
interface ConnectionListener {
    fun onState(state: ConnectionState)
    fun onWelcome(welcome: Welcome)
    fun onLayout(layout: Layout)
    fun onStatus(status: Status)
    /** The desktop said no; the connection stops trying. */
    fun onDenied(reason: String)
}

enum class ConnectionState { CONNECTING, CONNECTED, RECONNECTING, STOPPED }

/**
 * The phone's end: connects, says hello, and keeps trying every few
 * seconds whenever the desktop goes away, until [stop]. The desktop
 * stays in charge, so losing this never affects a recording there.
 */
class Connection(
    private val host: String,
    private val port: Int,
    private val phoneName: String,
    /** The pairing code, the first time. */
    private val code: String?,
    /** The token from last time's welcome, after that. */
    private var token: String?,
    private val listener: ConnectionListener,
) {
    private val stopped = AtomicBoolean(false)
    @Volatile private var writer: Writer? = null
    @Volatile private var socket: Socket? = null
    private val lock = Any()
    private var thread: Thread? = null

    fun start() {
        thread = Thread({ run() }, "daw-remote").also { it.isDaemon = true; it.start() }
    }

    fun stop() {
        stopped.set(true)
        closeSocket()
        listener.onState(ConnectionState.STOPPED)
    }

    /** Sends a request; nothing happens if the desktop is away. */
    fun send(msg: FromPhone) {
        val w = writer ?: return
        synchronized(lock) {
            try {
                w.write(Protocol.encode(msg))
                w.flush()
            } catch (e: IOException) {
                closeSocket()
            }
        }
    }

    private fun closeSocket() {
        try {
            socket?.close()
        } catch (_: IOException) {
        }
    }

    private fun run() {
        var attempt = 0
        var usedCode = false
        while (!stopped.get()) {
            listener.onState(if (attempt == 0) ConnectionState.CONNECTING else ConnectionState.RECONNECTING)
            val denied = try {
                session(if (usedCode) null else code).also { usedCode = true }
            } catch (e: IOException) {
                null
            }
            writer = null
            if (denied != null) {
                listener.onDenied(denied)
                stopped.set(true)
                listener.onState(ConnectionState.STOPPED)
                return
            }
            if (stopped.get()) return
            attempt++
            try {
                Thread.sleep(min(1000L * attempt, RETRY_MAX_MS))
            } catch (_: InterruptedException) {
                return
            }
        }
    }

    /** One connection, until it drops. Returns a denial's reason, if any. */
    private fun session(code: String?): String? {
        val s = Socket()
        socket = s
        s.tcpNoDelay = true
        s.connect(InetSocketAddress(host, port), CONNECT_TIMEOUT_MS)
        s.soTimeout = READ_TIMEOUT_MS
        val w = OutputStreamWriter(s.getOutputStream(), Charsets.UTF_8)
        val r = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.UTF_8), 1 shl 16)
        w.write(Protocol.encode(Hello(name = phoneName, code = code, token = token)))
        w.flush()
        var lastPing = System.currentTimeMillis()
        var welcomed = false
        try {
            while (!stopped.get()) {
                val line = try {
                    r.readLine() ?: return null
                } catch (e: java.net.SocketTimeoutException) {
                    // Nothing for a while: the desktop sends status many
                    // times a second, so it has gone.
                    return null
                }
                when (val msg = Protocol.decode(line)) {
                    is Welcome -> {
                        token = msg.token
                        welcomed = true
                        writer = w
                        listener.onWelcome(msg)
                        listener.onState(ConnectionState.CONNECTED)
                    }
                    is Denied -> return msg.reason
                    is Layout -> listener.onLayout(msg)
                    is Status -> listener.onStatus(msg)
                    null -> {}
                }
                if (welcomed && System.currentTimeMillis() - lastPing > PING_EVERY_MS) {
                    send(Ping)
                    lastPing = System.currentTimeMillis()
                }
            }
            return null
        } finally {
            writer = null
            try {
                s.close()
            } catch (_: IOException) {
            }
        }
    }

    companion object {
        const val CONNECT_TIMEOUT_MS = 4000
        /** The desktop sends status about 15 times a second. */
        const val READ_TIMEOUT_MS = 10_000
        const val PING_EVERY_MS = 5_000
        const val RETRY_MAX_MS = 5_000L
    }
}
