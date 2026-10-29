// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import org.daw.remote.ui.RemoteApp

class MainActivity : ComponentActivity() {
    private val vm: RemoteViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Stays awake while it is open; the system lets the screen turn
        // off as usual once the app is in the background.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContent { RemoteApp(vm) }
    }
}
