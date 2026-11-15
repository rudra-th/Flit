package dev.flit.android

import android.widget.Toast
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
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

/**
 * The whole app, one screen.
 *
 * The top card is this phone, because that is the thing a person needs to
 * compare with the other screen. Below it is whoever answered on this network.
 * There is no send button yet; a button that does nothing is worse than a
 * sentence that says what happens next.
 */
@Composable
fun HomeScreen(state: HomeState, onRefresh: () -> Unit) {
    val colors = FlitPalette.colors

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .windowInsetsPadding(WindowInsets.safeDrawing),
    ) {
        Header(refreshing = state.refreshing, onRefresh = onRefresh)

        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(
                start = 20.dp,
                end = 20.dp,
                bottom = 24.dp,
            ),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                val error = state.error
                val device = state.device
                when {
                    error != null -> Notice(text = error)
                    device == null -> Starting()
                    else -> Identity(device)
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
                    items(state.peers, key = { it.deviceId }) { peer -> Peer(peer) }
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

@Composable
private fun Identity(device: DeviceInfo) {
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
                Text(
                    text = device.alias,
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink2,
                )
            }
            Spacer(Modifier.height(10.dp))
            Text(
                text = device.emoji,
                fontSize = 26.sp,
                letterSpacing = 6.sp,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = device.words,
                fontFamily = FontFamily.Monospace,
                fontSize = 15.sp,
                fontWeight = FontWeight.Medium,
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
private fun Peer(peer: PeerInfo) {
    val colors = FlitPalette.colors

    Surface(
        modifier = Modifier.fillMaxWidth(),
        color = colors.card,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, colors.line),
    ) {
        Row(modifier = Modifier.padding(14.dp)) {
            Text(text = peer.emoji, fontSize = 18.sp, letterSpacing = 3.sp)
            Spacer(Modifier.width(12.dp))
            Column {
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
                    text = peer.addr,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 12.sp,
                    color = colors.ink2,
                )
                Spacer(Modifier.height(2.dp))
                Text(
                    text = peer.words,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.5.sp,
                    color = colors.ink2,
                )
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
