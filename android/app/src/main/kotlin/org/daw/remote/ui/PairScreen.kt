// SPDX-License-Identifier: GPL-3.0-or-later
package org.daw.remote.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import org.daw.remote.core.Pairing

/** Scan the code on the desktop (Settings → Phone remote…), or type it. */
@Composable
fun PairScreen(denied: String?, onPair: (Pairing) -> Unit) {
    val context = LocalContext.current
    var typed by remember { mutableStateOf("") }
    var problem by remember { mutableStateOf<String?>(null) }
    var scanning by remember { mutableStateOf(false) }
    var hasCamera by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val askCamera = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        hasCamera = granted
        scanning = granted
        if (!granted) problem = "Without the camera, type the code instead."
    }

    fun pairWith(text: String) {
        val p = Pairing.parse(text)
        if (p == null) {
            problem = "That isn't a pairing code. It starts with daw-remote://"
        } else {
            problem = null
            onPair(p)
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .padding(20.dp)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Pair with the desktop", style = MaterialTheme.typography.headlineMedium)
        Text(
            "On the computer, open Settings → Phone remote… and press " +
                "“Show a code to pair a phone”. Both must be on the same Wi-Fi.",
            style = MaterialTheme.typography.bodyLarge,
        )
        if (denied != null) {
            Text("The desktop said: $denied", color = MaterialTheme.colorScheme.error)
        }
        if (problem != null) {
            Text(problem!!, color = MaterialTheme.colorScheme.error)
        }
        if (scanning && hasCamera) {
            QrScanner(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(320.dp),
                onCode = { text ->
                    scanning = false
                    pairWith(text)
                },
            )
            OutlinedButton(onClick = { scanning = false }, modifier = Modifier.fillMaxWidth()) {
                Text("Stop scanning")
            }
        } else {
            Button(
                onClick = {
                    if (hasCamera) scanning = true else askCamera.launch(Manifest.permission.CAMERA)
                },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(64.dp),
            ) {
                Text("Scan the code", style = MaterialTheme.typography.titleLarge)
            }
        }
        Spacer(Modifier.height(8.dp))
        Text("Or type what is shown under the code:", style = MaterialTheme.typography.bodyMedium)
        OutlinedTextField(
            value = typed,
            onValueChange = { typed = it },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            placeholder = { Text("daw-remote://192.168.1.20:7345/ABCD…") },
        )
        OutlinedButton(
            onClick = { pairWith(typed) },
            enabled = typed.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Connect")
        }
    }
    LaunchedEffect(Unit) {
        // Start on the camera when it is already allowed.
        if (hasCamera) scanning = true
    }
}
