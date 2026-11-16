package dev.flit.android

import android.net.Uri
import android.text.format.Formatter
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.flit.android.ffi.DeviceInfo
import dev.flit.android.ffi.PeerInfo
import dev.flit.android.ffi.ReceivedFile

/**
 * The whole app, one screen.
 *
 * The top card is this phone, because that is the thing a person needs to
 * compare with the other screen. Below it is whoever answered on this network;
 * tapping one picks the destination and the bar at the bottom offers the file
 * picker. A send has one row above the list: a bar, a count, and a way out.
 */
@Composable
fun HomeScreen(
    state: HomeState,
    onRefresh: () -> Unit,
    onRename: (String) -> Unit,
    onNoticeShown: () -> Unit,
    onSelect: (String) -> Unit,
    onSend: (List<Uri>) -> Unit,
    onCancelSend: () -> Unit,
    onDismissTransfer: () -> Unit,
) {
    val colors = FlitPalette.colors
    val context = LocalContext.current
    var renaming by remember { mutableStateOf(false) }

    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenMultipleDocuments(),
    ) { uris -> if (uris.isNotEmpty()) onSend(uris) }

    // Renaming restarts the receiver, so the sentence arrives after the engine
    // is already coming back up. A toast keeps it out of the screen's layout.
    LaunchedEffect(state.notice) {
        val notice = state.notice ?: return@LaunchedEffect
        Toast.makeText(context, notice, Toast.LENGTH_SHORT).show()
        onNoticeShown()
    }

    if (renaming) {
        RenameDialog(
            current = state.device?.alias.orEmpty(),
            onDismiss = { renaming = false },
            onSave = { name ->
                renaming = false
                onRename(name)
            },
        )
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .windowInsetsPadding(WindowInsets.safeDrawing),
    ) {
        Header(refreshing = state.refreshing, onRefresh = onRefresh)

        LazyColumn(
            modifier = Modifier.weight(1f),
            contentPadding = PaddingValues(
                start = 20.dp,
                end = 20.dp,
                bottom = 16.dp,
            ),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            val transfer = state.transfer
            if (transfer != null) {
                item { TransferCard(transfer, onCancel = onCancelSend, onDismiss = onDismissTransfer) }
            }

            item {
                val error = state.error
                val device = state.device
                when {
                    error != null -> Notice(text = error)
                    device == null -> Starting()
                    else -> Identity(device, onChangeName = { renaming = true })
                }
            }

            if (state.device != null) {
                item {
                    Spacer(Modifier.height(8.dp))
                    NearbyHeader(count = state.peers.size)
                }

                if (state.peers.isEmpty()) {
                    item { Empty(onNetwork = state.device.discovery == "running") }
                } else {
                    items(state.peers, key = { it.deviceId }) { peer ->
                        Peer(
                            peer = peer,
                            selected = peer.deviceId == state.selected,
                            onClick = { onSelect(peer.deviceId) },
                        )
                    }
                }

                if (state.received.isNotEmpty()) {
                    item {
                        Spacer(Modifier.height(8.dp))
                        ReceivedHeader(count = state.received.size)
                    }
                    items(state.received, key = { it.name + it.modified }) { file ->
                        ReceivedRow(file)
                    }
                }

                item {
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text = "Anyone on this Wi\u2011Fi can see this phone. Nothing leaves the network.",
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.ink2,
                        modifier = Modifier.padding(horizontal = 4.dp),
                    )
                }
            }
        }

        val chosen = state.peers.firstOrNull { it.deviceId == state.selected }
        if (chosen != null) {
            SendBar(
                deviceName = chosen.alias,
                enabled = state.device != null,
                onPick = { picker.launch(arrayOf("*/*")) },
            )
        }
    }
}

@Composable
private fun Header(refreshing: Boolean, onRefresh: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 20.dp, end = 12.dp, top = 20.dp, bottom = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text("Flit", style = MaterialTheme.typography.displaySmall)
            Text(
                text = "Files that stay on your network.",
                style = MaterialTheme.typography.bodyMedium,
                color = FlitPalette.colors.ink2,
            )
        }

        TextButton(onClick = onRefresh, enabled = !refreshing) {
            if (refreshing) {
                CircularProgressIndicator(
                    modifier = Modifier.size(14.dp),
                    strokeWidth = 2.dp,
                    color = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.width(8.dp))
                Text("Looking")
            } else {
                Text("Refresh")
            }
        }
    }
}

/** The send in flight, or the one that just ended. */
@Composable
private fun TransferCard(
    transfer: TransferUi,
    onCancel: () -> Unit,
    onDismiss: () -> Unit,
) {
    val colors = FlitPalette.colors
    val context = LocalContext.current
    val size = { bytes: Long -> Formatter.formatShortFileSize(context, bytes) }

    val headline = when {
        transfer.problem != null && transfer.finished -> "${transfer.filesDone} of ${transfer.files} saved"
        transfer.problem != null -> "Send failed"
        transfer.cancelled -> "Cancelled"
        transfer.finished -> "Sent"
        else -> transfer.label.ifEmpty { "Sending" }
    }

    val detail = when {
        transfer.problem != null -> transfer.problem
        transfer.cancelled -> "Nothing else will be sent."
        transfer.finished -> "${transfer.filesDone} of ${transfer.files} files \u00b7 ${size(transfer.total)}"
        transfer.total > 0L ->
            "${size(transfer.sent)} of ${size(transfer.total)} \u00b7 ${transfer.filesDone} of ${transfer.files}"
        else -> "Preparing\u2026"
    }

    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = colors.card,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, colors.line),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = headline,
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = if (transfer.done) onDismiss else onCancel) {
                    Text(if (transfer.done) "Done" else "Cancel")
                }
            }

            if (!transfer.done) {
                Spacer(Modifier.height(8.dp))
                LinearProgressIndicator(
                    progress = { transfer.fraction },
                    modifier = Modifier.fillMaxWidth(),
                    color = MaterialTheme.colorScheme.primary,
                )
            }

            Spacer(Modifier.height(8.dp))
            Text(
                text = detail,
                style = MaterialTheme.typography.bodySmall,
                color = colors.ink2,
            )
        }
    }
}

@Composable
private fun Identity(device: DeviceInfo, onChangeName: () -> Unit) {
    val colors = FlitPalette.colors
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current

    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = colors.accentSoft,
        shape = RoundedCornerShape(20.dp),
        border = BorderStroke(1.dp, colors.accentLine),
    ) {
        Column(modifier = Modifier.padding(20.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = "THIS PHONE",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.weight(1f))
                TextButton(onClick = onChangeName) {
                    Text("Change")
                }
            }
            Text(
                text = device.alias,
                fontSize = 19.sp,
                fontWeight = FontWeight.Medium,
                letterSpacing = 0.2.sp,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                text = shortFingerprint(device.fingerprint),
                fontFamily = FontFamily.Monospace,
                fontSize = 11.5.sp,
                color = colors.ink2,
            )
            Spacer(Modifier.height(12.dp))
            HorizontalDivider(color = colors.accentLine.copy(alpha = 0.7f))
            Spacer(Modifier.height(10.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = device.addr,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.5.sp,
                    color = colors.ink2,
                    modifier = Modifier.weight(1f),
                )
                TextButton(
                    onClick = {
                        clipboard.setText(AnnotatedString(device.addr))
                        Toast.makeText(context, "Address copied", Toast.LENGTH_SHORT).show()
                    },
                ) {
                    Text("Copy")
                }
            }
        }
    }
}

/** The first 16 hex characters, as a person would read them off two screens. */
internal fun shortFingerprint(fingerprint: String): String =
    fingerprint.take(16).chunked(4).joinToString(" ")

/** Editing the name is a label change, not an identity change: say so once. */
@Composable
private fun RenameDialog(
    current: String,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
) {
    var draft by remember(current) { mutableStateOf(current) }
    val cleaned = draft.trim().split(Regex("\\s+")).joinToString(" ")

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Name this phone") },
        text = {
            Column {
                Text(
                    text = "Other devices show this name. Anyone can type any name, so " +
                        "Flit pins the fingerprint, not the name.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = FlitPalette.colors.ink2,
                )
                Spacer(Modifier.height(14.dp))
                OutlinedTextField(
                    value = draft,
                    onValueChange = { if (it.length <= MAX_NAME_CHARS) draft = it },
                    singleLine = true,
                    label = { Text("Name") },
                    supportingText = { Text("${cleaned.length}/$MAX_NAME_CHARS") },
                )
            }
        },
        confirmButton = {
            TextButton(
                enabled = cleaned.isNotEmpty(),
                onClick = { onSave(cleaned) },
            ) {
                Text("Save")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

/** Longest name the engine will announce. */
private const val MAX_NAME_CHARS = 32

@Composable
private fun NearbyHeader(count: Int) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            text = "NEARBY",
            style = MaterialTheme.typography.labelSmall,
            color = FlitPalette.colors.ink2,
        )
        Spacer(Modifier.weight(1f))
        Text(
            text = when (count) {
                0 -> "nothing yet"
                1 -> "1 device"
                else -> "$count devices"
            },
            style = MaterialTheme.typography.bodySmall,
            color = FlitPalette.colors.ink2,
        )
    }
}

@Composable
private fun Peer(peer: PeerInfo, selected: Boolean, onClick: () -> Unit) {
    val colors = FlitPalette.colors

    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
        color = if (selected) colors.accentSoft else colors.card,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(
            1.dp,
            if (selected) colors.accentLine else colors.line,
        ),
    ) {
        Row(modifier = Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        text = peer.alias,
                        style = MaterialTheme.typography.titleMedium,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    Spacer(Modifier.width(8.dp))
                    Text(
                        text = peer.deviceType,
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.ink2,
                    )
                }
                Text(
                    text = "${shortFingerprint(peer.fingerprint)} \u00b7 ${peer.addr}",
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.5.sp,
                    color = colors.ink2,
                )
            }

            if (selected) {
                Spacer(Modifier.width(10.dp))
                Text(
                    text = "Chosen",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}

@Composable
private fun ReceivedHeader(count: Int) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            text = "RECEIVED",
            style = MaterialTheme.typography.labelSmall,
            color = FlitPalette.colors.ink2,
        )
        Spacer(Modifier.weight(1f))
        Text(
            text = if (count == 1) "1 file" else "$count files",
            style = MaterialTheme.typography.bodySmall,
            color = FlitPalette.colors.ink2,
        )
    }
}

@Composable
private fun ReceivedRow(file: ReceivedFile) {
    val colors = FlitPalette.colors
    val context = LocalContext.current

    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = colors.card,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, colors.line),
    ) {
        Row(modifier = Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = file.name,
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    text = buildString {
                        append(Formatter.formatShortFileSize(context, file.bytes.toLong()))
                        if (file.folder) append(" \u00b7 folder")
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink2,
                )
            }
        }
    }
}

/** Bottom bar: the destination is chosen, so offer the picker. */
@Composable
private fun SendBar(deviceName: String, enabled: Boolean, onPick: () -> Unit) {
    val colors = FlitPalette.colors

    Surface(color = colors.card, border = BorderStroke(1.dp, colors.line)) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 20.dp, end = 16.dp, top = 12.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text("To", style = MaterialTheme.typography.labelSmall, color = colors.ink2)
                Text(
                    text = deviceName,
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                )
            }
            Button(onClick = onPick, enabled = enabled) {
                Text("Choose files")
            }
        }
    }
}

@Composable
private fun Empty(onNetwork: Boolean) {
    val colors = FlitPalette.colors

    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = colors.card,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, colors.line),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 24.dp, vertical = 26.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                text = "No one yet",
                style = MaterialTheme.typography.titleMedium,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                text = if (onNetwork) {
                    "Open Flit on a computer on this Wi\u2011Fi. It announces itself; there is nothing to type."
                } else {
                    "This phone is not hearing announcements, so nothing will appear on its own."
                },
                style = MaterialTheme.typography.bodyMedium,
                color = colors.ink2,
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun Starting() {
    val colors = FlitPalette.colors

    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = colors.card,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, colors.line),
    ) {
        Row(
            modifier = Modifier.padding(18.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            CircularProgressIndicator(
                modifier = Modifier.size(16.dp),
                strokeWidth = 2.dp,
                color = MaterialTheme.colorScheme.primary,
            )
            Spacer(Modifier.width(12.dp))
            Text(
                text = "Opening the receiver\u2026",
                style = MaterialTheme.typography.bodyMedium,
                color = colors.ink2,
            )
        }
    }
}

@Composable
private fun Notice(text: String) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = MaterialTheme.colorScheme.errorContainer,
        shape = RoundedCornerShape(14.dp),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = "The receiver did not start",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = text,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onErrorContainer,
            )
        }
    }
}
