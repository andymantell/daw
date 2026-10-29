// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.ui

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.daw.remote.RemoteViewModel

/** Dark, high contrast: it is read from a stool at arm's length. */
val Scheme: ColorScheme = darkColorScheme(
    primary = Color(0xFF8BB8FF),
    onPrimary = Color(0xFF0B1A33),
    secondary = Color(0xFFB6C2D0),
    background = Color(0xFF101418),
    surface = Color(0xFF161B21),
    surfaceVariant = Color(0xFF222932),
    onSurface = Color(0xFFE6EDF3),
    error = Color(0xFFF0463C),
)

val RecordRed = Color(0xFFF0463C)
val TakeGreen = Color(0xFF59C27A)
val FileBlue = Color(0xFF5A8FD6)

@Composable
fun RemoteApp(vm: RemoteViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    MaterialTheme(colorScheme = Scheme) {
        Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            if (state.desktopAddress == null || state.denied != null) {
                PairScreen(denied = state.denied, onPair = vm::pair)
            } else {
                RemoteScreen(state = state, send = vm::send, onForget = vm::forget)
            }
        }
    }
}
