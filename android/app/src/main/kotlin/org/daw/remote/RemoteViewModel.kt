// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote

import android.app.Application
import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.lifecycle.AndroidViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.daw.remote.core.Connection
import org.daw.remote.core.ConnectionListener
import org.daw.remote.core.ConnectionState
import org.daw.remote.core.FromPhone
import org.daw.remote.core.Layout
import org.daw.remote.core.Pairing
import org.daw.remote.core.Status
import org.daw.remote.core.Welcome

/** What the app knows about the desktop, for the screens to draw. */
data class Remote(
    /** Null until a code has been scanned. */
    val desktopAddress: String? = null,
    val desktopName: String? = null,
    val connection: ConnectionState? = null,
    val layout: Layout? = null,
    val status: Status? = null,
    /** The desktop turned the phone away; it needs a new code. */
    val denied: String? = null,
)

/**
 * Keeps the connection across screen turns and remembers the desktop
 * (its address and this phone's token) between runs.
 */
class RemoteViewModel(app: Application) : AndroidViewModel(app) {
    private val prefs = app.getSharedPreferences("remote", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(Remote())
    val state: StateFlow<Remote> = _state
    private var connection: Connection? = null

    init {
        val host = prefs.getString("host", null)
        val token = prefs.getString("token", null)
        if (host != null && token != null) {
            connect(host, prefs.getInt("port", 0), code = null, token = token)
        }
    }

    /** Starts afresh with a code from the desktop's screen. */
    fun pair(p: Pairing) {
        connection?.stop()
        prefs.edit().putString("host", p.host).putInt("port", p.port).remove("token").apply()
        _state.value = Remote()
        connect(p.host, p.port, code = p.code, token = null)
    }

    /** Drops the desktop; the next start asks for a code again. */
    fun forget() {
        connection?.stop()
        connection = null
        prefs.edit().clear().apply()
        _state.value = Remote()
    }

    fun send(msg: FromPhone) {
        connection?.send(msg)
    }

    private fun connect(host: String, port: Int, code: String?, token: String?) {
        _state.value = _state.value.copy(desktopAddress = "$host:$port", connection = ConnectionState.CONNECTING)
        val listener = object : ConnectionListener {
            override fun onState(state: ConnectionState) {
                _state.value = _state.value.copy(connection = state)
            }

            override fun onWelcome(welcome: Welcome) {
                prefs.edit().putString("token", welcome.token).apply()
                _state.value = _state.value.copy(desktopName = welcome.desktop, denied = null)
            }

            override fun onLayout(layout: Layout) {
                _state.value = _state.value.copy(layout = layout)
            }

            override fun onStatus(status: Status) {
                _state.value = _state.value.copy(status = status)
            }

            override fun onDenied(reason: String) {
                prefs.edit().remove("token").apply()
                _state.value = _state.value.copy(denied = reason, connection = ConnectionState.STOPPED)
            }
        }
        connection = Connection(host, port, phoneName(), code, token, listener).also { it.start() }
    }

    private fun phoneName(): String {
        val named = try {
            Settings.Global.getString(getApplication<Application>().contentResolver, Settings.Global.DEVICE_NAME)
        } catch (e: SecurityException) {
            null
        }
        return named?.takeIf { it.isNotBlank() } ?: "${Build.MANUFACTURER} ${Build.MODEL}".trim()
    }

    override fun onCleared() {
        connection?.stop()
    }
}
