package dev.flit.android

import android.content.pm.PackageManager
import android.net.Uri
import android.os.StatFs
import android.text.format.Formatter
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.flit.android.ffi.DeviceInfo
import dev.flit.android.ffi.OfferEvent
import dev.flit.android.ffi.PeerInfo
import dev.flit.android.ffi.ReceivedFile
import java.io.File
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/** The three things this app does, as the reference learned and as people expect. */
private enum class FlitTab(val label: String) {
    Receive("Receive"),
    Send("Send"),
    Settings("Settings"),
}

/**
 * The whole app, one activity, three tabs.
 *
 * Receive is the identity surface: this phone, its name, its proof, and what
 * has landed. Send is the working surface: devices one tap away from the file
 * picker. Settings is where the label and the address live. Nothing here
 * requires a settings visit to work.
 */
@Composable
fun HomeScreen(
    state: HomeState,
    onRefresh: () -> Unit,
    onRename: (String) -> Unit,
    onNoticeShown: () -> Unit,
    onChoose: (String) -> Unit,
    onSend: (List<Uri>) -> Unit,
    onCancelSend: () -> Unit,
    onDismissTransfer: () -> Unit,
    onUseAddress: (String, String) -> Unit,
    onClearTarget: () -> Unit,
    onRemoveReceived: (ReceivedFile) -> Unit,
    onUndoRemove: () -> Unit,
    onAnswerOffer: (List<String>, Boolean) -> Unit,
    onSetAutoAccept: (Boolean) -> Unit,
    onForgetTrusted: (String) -> Unit,
) {
    val colors = FlitPalette.colors
    val context = LocalContext.current
    var tab by rememberSaveable { mutableStateOf(FlitTab.Receive.name) }
    var renaming by remember { mutableStateOf(false) }
    var addressing by remember { mutableStateOf(false) }
    /** The ledger row whose actions are on the table, if any. */
    var fileActions by remember { mutableStateOf<ReceivedFile?>(null) }
    val snacks = remember { SnackbarHostState() }

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

    // The undo offer lives as long as the removed row does: undo restores the
    // list, the timeout deletes the file, and either clears [removed]. The
    // action's result is what restores — showSnackbar returns only after the
    // tap, and nothing else would call [onUndoRemove].
    LaunchedEffect(state.removed) {
        val removed = state.removed
        if (removed == null) {
            snacks.currentSnackbarData?.dismiss()
            return@LaunchedEffect
        }
        val result = snacks.showSnackbar(
            message = "Deleted " + removed.name,
            actionLabel = "Undo",
            duration = SnackbarDuration.Indefinite,
        )
        if (result == SnackbarResult.ActionPerformed) {
            onUndoRemove()
        }
    }

    val active = FlitTab.valueOf(tab)

    Surface(color = MaterialTheme.colorScheme.background) {
        Column(modifier = Modifier.fillMaxSize()) {
            TopBar(
                refreshing = state.refreshing && active == FlitTab.Send,
                showRefresh = active == FlitTab.Send,
                onRefresh = onRefresh,
            )

            Column(modifier = Modifier.weight(1f)) {
                when (active) {
                    FlitTab.Receive -> ReceiveTab(
                        state = state,
                        onActions = { fileActions = it },
                    )
                    FlitTab.Send -> SendTab(
                        state = state,
                        onChoose = onChoose,
                        onPick = { picker.launch(arrayOf("*/*")) },
                        onCancelSend = onCancelSend,
                        onDismissTransfer = onDismissTransfer,
                        onAddress = { addressing = true },
                        onClearTarget = onClearTarget,
                    )
                    FlitTab.Settings -> SettingsTab(
                        state = state,
                        onRename = { renaming = true },
                        onSetAutoAccept = onSetAutoAccept,
                        onForgetTrusted = onForgetTrusted,
                    )
                }
            }

            SnackbarHost(
                hostState = snacks,
                modifier = Modifier.padding(horizontal = 12.dp),
            )

            BottomBar(active = active, onSelect = { tab = it.name })
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

        if (addressing) {
            AddressDialog(
                onDismiss = { addressing = false },
                onUse = { addr, fingerprint ->
                    addressing = false
                    onUseAddress(addr, fingerprint)
                },
            )
        }

        fileActions?.let { file ->
            FileActionsDialog(
                file = file,
                onDismiss = { fileActions = null },
                onShare = {
                    fileActions = null
                    shareReceived(context, resolveReceived(context, file.name))
                },
                onDelete = {
                    fileActions = null
                    onRemoveReceived(file)
                },
            )
        }

        state.offer?.let { offer ->
            OfferSheet(
                offer = offer,
                onAccept = { ids, remember -> onAnswerOffer(ids, remember) },
                onDecline = { onAnswerOffer(emptyList(), false) },
            )
        }
    }
}

/**
 * Wordmark, and the look-again control where it is meaningful. The identity
 * chip is gone: this phone lives on Receive and in Settings now.
 */
@Composable
private fun TopBar(
    refreshing: Boolean,
    showRefresh: Boolean,
    onRefresh: () -> Unit,
) {
    val colors = FlitPalette.colors

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .statusBarsPadding()
            .height(60.dp)
            .padding(start = 20.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "Flit",
            style = MaterialTheme.typography.headlineSmall,
        )
        Spacer(Modifier.weight(1f))

        if (showRefresh) {
            IconTap(
                label = if (refreshing) "Looking for devices" else "Look again",
                onClick = onRefresh,
                enabled = !refreshing,
            ) {
                if (refreshing) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(15.dp),
                        strokeWidth = 1.8.dp,
                        color = MaterialTheme.colorScheme.primary,
                    )
                } else {
                    RefreshGlyph(tint = colors.ink2)
                }
            }
        }
    }
}

/** A 48 dp tap target around a 24 dp glyph: the icon is small, the target is not. */
@Composable
private fun IconTap(
    label: String,
    onClick: () -> Unit,
    enabled: Boolean = true,
    content: @Composable () -> Unit,
) {
    Box(
        modifier = Modifier
            .size(48.dp)
            .clickable(enabled = enabled, onClickLabel = label, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        content()
    }
}

// ---------------------------------------------------------------- Receive --

/**
 * This phone, first: who it is and what it has received. The avatar is the
 * ink circle with the name's initials — the one place the app draws itself
 * as a thing, because this is the screen other devices send to.
 */
@Composable
private fun ReceiveTab(state: HomeState, onActions: (ReceivedFile) -> Unit) {
    val colors = FlitPalette.colors

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = 20.dp, end = 20.dp, bottom = 24.dp),
    ) {
        item(key = "hero") {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Avatar(alias = state.device?.alias)
                Spacer(Modifier.height(14.dp))
                Text(
                    text = state.device?.alias ?: "Starting\u2026",
                    style = MaterialTheme.typography.headlineSmall,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    text = "phone \u00b7 " + shortFingerprint(state.device?.fingerprint.orEmpty()),
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink2,
                )
            }
        }

        item(key = "note") {
            Text(
                text = "Anyone on this Wi\u2011Fi can see this phone. Nothing leaves " +
                    "the network.",
                style = MaterialTheme.typography.bodySmall,
                color = colors.ink3,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 10.dp, start = 24.dp, end = 24.dp),
            )
        }

        item(key = "received") {
            SectionHeading(
                title = "Received",
                trailing = when {
                    state.received.isEmpty() -> null
                    state.received.size == 1 -> "1 file"
                    else -> "${state.received.size} files"
                },
            )
        }

        if (state.received.isEmpty()) {
            item(key = "empty-received") {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 4.dp, bottom = 8.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(
                        text = "Nothing yet",
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text = "Files sent to ${state.device?.alias ?: "this phone"} " +
                            "land here and stay on the phone.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = colors.ink2,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(horizontal = 20.dp),
                    )
                }
            }
        } else {
            ledgerRows(state.received).forEach { row ->
                when (row) {
                    is LedgerRow.Day -> item(key = "day${row.label}") {
                        DayDivider(row.label)
                    }
                    is LedgerRow.File -> item(
                        key = "file${row.file.name}${row.file.modified}",
                    ) {
                        ReceivedRow(row.file, onActions = onActions)
                    }
                }
            }
        }
    }
}

private sealed interface LedgerRow {
    data class Day(val label: String) : LedgerRow
    data class File(val file: ReceivedFile) : LedgerRow
}

/** Received files, grouped under the day they landed, newest day first. */
private fun ledgerRows(files: List<ReceivedFile>): List<LedgerRow> {
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    val rows = mutableListOf<LedgerRow>()
    var lastDay: LocalDate? = null
    for (file in files.sortedByDescending { it.modified }) {
        val day = Instant.ofEpochSecond(file.modified.toLong()).atZone(zone).toLocalDate()
        if (day != lastDay) {
            rows += LedgerRow.Day(
                when (day) {
                    today -> "Today"
                    today.minusDays(1) -> "Yesterday"
                    else -> day.format(DateTimeFormatter.ofPattern("d MMM"))
                },
            )
            lastDay = day
        }
        rows += LedgerRow.File(file)
    }
    return rows
}

/** The ink circle with the name's initials. */
@Composable
private fun Avatar(alias: String?) {
    val ink = MaterialTheme.colorScheme.onBackground
    val paper = MaterialTheme.colorScheme.background
    val initials = alias.orEmpty()
        .split(Regex("\\s+"))
        .filter { it.isNotEmpty() }
        .take(2)
        .joinToString("") { it.first().uppercase() }
        .ifEmpty { "?" }

    Box(
        modifier = Modifier
            .size(96.dp)
            .background(ink, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = initials,
            style = MaterialTheme.typography.displaySmall,
            color = paper,
        )
    }
}

// ------------------------------------------------------------------- Send --

/**
 * Devices are the content: a list of names, each row one tap away from the
 * file picker. A chosen device is an ink band — the only dark surface on the
 * paper, so the eye lands on it first and the check confirms it.
 */
@Composable
private fun SendTab(
    state: HomeState,
    onChoose: (String) -> Unit,
    onPick: () -> Unit,
    onCancelSend: () -> Unit,
    onDismissTransfer: () -> Unit,
    onAddress: () -> Unit,
    onClearTarget: () -> Unit,
) {
    val colors = FlitPalette.colors

    val chosen = state.peers.firstOrNull { it.deviceId == state.selected }
    val target = chosen?.alias ?: state.manual?.addr
    // The slab is for choosing files. While a send is in flight the transfer
    // panel replaces it, so only one thing asks for attention at a time.
    val slabVisible = target != null && state.transfer == null

    Column(modifier = Modifier.fillMaxSize()) {
        LazyColumn(
            modifier = Modifier.weight(1f),
            contentPadding = PaddingValues(start = 20.dp, end = 20.dp, bottom = 16.dp),
        ) {
            val transfer = state.transfer
            if (transfer != null) {
                item(key = "transfer") {
                    TransferPanel(
                        transfer = transfer,
                        onCancel = onCancelSend,
                        onDismiss = onDismissTransfer,
                    )
                }
            }

            if (state.error != null) {
                item(key = "error") {
                    ErrorPanel(message = state.error, onRetry = onPick)
                }
            }

            if (state.device == null) {
                if (state.error == null) {
                    item(key = "starting") { StartingRow() }
                }
            } else {
                item(key = "nearby") {
                    SectionHeading(
                        title = "Nearby",
                        trailing = when {
                            state.refreshing && state.peers.isEmpty() -> "looking"
                            state.peers.isEmpty() -> "nothing yet"
                            state.peers.size == 1 -> "1 device"
                            else -> "${state.peers.size} devices"
                        },
                    )
                }

                if (state.peers.isEmpty()) {
                    item(key = "empty") {
                        Empty(discovering = state.device.discovery == "running")
                    }
                } else {
                    items(state.peers, key = { it.deviceId }) { peer ->
                        PeerRow(
                            peer = peer,
                            selected = peer.deviceId == state.selected,
                            onClick = {
                                onChoose(peer.deviceId)
                                onPick()
                            },
                        )
                    }
                }

                item(key = "address") {
                    QuietAction(
                        label = "Send to an address",
                        onClick = onAddress,
                    )
                }
            }
        }

        AnimatedVisibility(
            visible = slabVisible,
            enter = slideInVertically { it } + fadeIn(),
            exit = slideOutVertically { it } + fadeOut(),
        ) {
            SendSlab(
                target = target.orEmpty(),
                typed = chosen == null,
                enabled = state.device != null,
                onPick = onPick,
                onClear = onClearTarget,
            )
        }
    }
}

/** A section's name in the display face, its count quiet at the right. */
@Composable
private fun SectionHeading(title: String, trailing: String?) {
    val colors = FlitPalette.colors

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 18.dp, bottom = 12.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        Text(
            text = title,
            style = MaterialTheme.typography.headlineSmall,
        )
        Spacer(Modifier.weight(1f))
        if (trailing != null) {
            Text(
                text = trailing,
                style = MaterialTheme.typography.bodySmall,
                color = colors.ink3,
                modifier = Modifier.padding(bottom = 4.dp),
            )
        }
    }
}

@Composable
private fun PeerRow(peer: PeerInfo, selected: Boolean, onClick: () -> Unit) {
    val colors = FlitPalette.colors
    val ink = MaterialTheme.colorScheme.onBackground
    val band by animateColorAsState(
        targetValue = if (selected) ink else Color.Transparent,
        animationSpec = tween(180),
        label = "peer-band",
    )
    val titleColor by animateColorAsState(
        targetValue = if (selected) colors.paper else MaterialTheme.colorScheme.onBackground,
        animationSpec = tween(180),
        label = "peer-title",
    )
    val metaColor by animateColorAsState(
        targetValue = if (selected) colors.paper.copy(alpha = 0.62f) else colors.ink3,
        animationSpec = tween(180),
        label = "peer-meta",
    )

    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 4.dp)
            .clickable(onClickLabel = "Send files to ${peer.alias}", onClick = onClick),
        color = band,
        shape = RoundedCornerShape(18.dp),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            DeviceGlyph(
                kind = peer.deviceType,
                tint = if (selected) colors.paper else colors.ink2,
            )
            Spacer(Modifier.width(16.dp))

            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = peer.alias,
                    style = MaterialTheme.typography.titleLarge,
                    color = titleColor,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Spacer(Modifier.height(3.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        text = peer.deviceType,
                        style = MaterialTheme.typography.bodySmall,
                        color = metaColor,
                    )
                    Dot(color = metaColor)
                    Text(
                        text = shortFingerprint(peer.fingerprint),
                        fontFamily = FontFamily.Monospace,
                        fontSize = 11.5.sp,
                        letterSpacing = 0.3.sp,
                        color = metaColor,
                    )
                }
            }

            AnimatedVisibility(
                visible = selected,
                enter = scaleIn(tween(160)) + fadeIn(tween(160)),
                exit = fadeOut(tween(120)),
            ) {
                Box(modifier = Modifier.padding(start = 10.dp)) {
                    CheckBadge()
                }
            }
        }
    }
}

/** A tick drawn rather than bundled: one icon is not worth an icon artifact. */
@Composable
private fun CheckBadge() {
    Box(
        modifier = Modifier
            .size(26.dp)
            .background(MaterialTheme.colorScheme.primary, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(13.dp)) {
            val stroke = 1.9.dp.toPx()
            drawLine(
                color = Color.White,
                start = Offset(size.width * 0.08f, size.height * 0.55f),
                end = Offset(size.width * 0.38f, size.height * 0.84f),
                strokeWidth = stroke,
                cap = StrokeCap.Round,
            )
            drawLine(
                color = Color.White,
                start = Offset(size.width * 0.38f, size.height * 0.84f),
                end = Offset(size.width * 0.94f, size.height * 0.2f),
                strokeWidth = stroke,
                cap = StrokeCap.Round,
            )
        }
    }
}

/**
 * The send in flight, or the one that just ended, in the same ink slab the
 * send bar uses — the bar does not disappear, it becomes the transfer.
 */
@Composable
private fun TransferPanel(
    transfer: TransferUi,
    onCancel: () -> Unit,
    onDismiss: () -> Unit,
) {
    val colors = FlitPalette.colors
    val context = LocalContext.current
    val size = { bytes: Long -> Formatter.formatShortFileSize(context, bytes) }
    val slab = MaterialTheme.colorScheme.onBackground
    val paperText = colors.paper
    // On the ink slab the theme's light-mode error is too dark to read, so the
    // danger colour the dark theme uses — light enough for ink — is the one
    // the slab borrows.
    val slabDanger = Color(0xFFFF8A80)

    val headline = when {
        transfer.problem != null && transfer.finished -> "Partly sent"
        transfer.problem != null -> "Failed"
        transfer.cancelled -> "Cancelled"
        transfer.finished -> "Sent"
        else -> "Sending"
    }

    val detail = when {
        transfer.problem != null && transfer.finished ->
            "${transfer.filesDone} of ${transfer.files} files made it to the other side."
        transfer.problem != null -> transfer.problem.orEmpty()
        transfer.cancelled -> "Stopped. Nothing else was sent."
        transfer.finished ->
            "${transfer.filesDone} of ${transfer.files} files \u00b7 ${size(transfer.total)}"
        transfer.total > 0L && transfer.sent == 0L ->
            "Waiting for the other device to accept\u2026"
        transfer.total > 0L ->
            "${size(transfer.sent)} of ${size(transfer.total)} \u00b7 " +
                "${transfer.filesDone} of ${transfer.files} files"
        else -> "Getting ready\u2026"
    }

    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 18.dp, bottom = 4.dp),
        color = slab,
        shape = RoundedCornerShape(24.dp),
    ) {
        Column(modifier = Modifier.padding(start = 22.dp, end = 10.dp, top = 18.dp, bottom = 12.dp)) {
            Text(
                text = headline,
                style = MaterialTheme.typography.headlineSmall,
                color = paperText,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                text = detail,
                style = MaterialTheme.typography.bodySmall,
                color = if (transfer.problem != null && !transfer.finished) {
                    slabDanger
                } else {
                    paperText.copy(alpha = 0.66f)
                },
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )

            if (!transfer.done) {
                Spacer(Modifier.height(16.dp))
                ProgressTrack(
                    fraction = transfer.fraction,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(end = 12.dp),
                )
            }

            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.End,
            ) {
                TextButton(onClick = if (transfer.done) onDismiss else onCancel) {
                    Text(
                        text = if (transfer.done) "Done" else "Cancel",
                        color = paperText,
                    )
                }
            }
        }
    }
}

/** A 6 dp track whose fill eases toward the last reported fraction. */
@Composable
private fun ProgressTrack(fraction: Float, modifier: Modifier = Modifier) {
    val colors = FlitPalette.colors
    val shown by animateFloatAsState(
        targetValue = fraction.coerceIn(0f, 1f),
        animationSpec = tween(durationMillis = 400, easing = LinearEasing),
        label = "send-progress",
    )
    val fill = colors.paper

    Box(
        modifier = modifier
            .height(6.dp)
            .background(fill.copy(alpha = 0.16f), CircleShape),
    ) {
        if (shown > 0f) {
            Box(
                modifier = Modifier
                    .fillMaxWidth(shown)
                    .height(6.dp)
                    .background(fill, CircleShape),
            )
        }
    }
}

/**
 * Nobody here yet: two machines and a dotted path between them, the near one
 * in the accent because this phone is the near one.
 */
@Composable
private fun Empty(discovering: Boolean) {
    val colors = FlitPalette.colors

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 28.dp, bottom = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        EmptyGlyph(
            near = MaterialTheme.colorScheme.primary,
            far = colors.ink3,
        )
        Spacer(Modifier.height(18.dp))
        Text(
            text = "No one here yet",
            style = MaterialTheme.typography.headlineSmall,
        )
        Spacer(Modifier.height(8.dp))
        Text(
            text = if (discovering) {
                "Open Flit on a computer on this Wi\u2011Fi. It announces itself; " +
                    "there is nothing to type."
            } else {
                "This phone is not hearing announcements, so nothing will appear " +
                    "on its own. Use an address instead."
            },
            style = MaterialTheme.typography.bodyMedium,
            color = colors.ink2,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(horizontal = 16.dp),
        )
    }
}

/** A quiet footer action: text, 48 dp tall, no fill, no outline. */
@Composable
private fun QuietAction(label: String, onClick: () -> Unit) {
    val colors = FlitPalette.colors

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 6.dp)
            .heightIn(min = 48.dp)
            .clickable(onClick = onClick),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = "+  $label",
            style = MaterialTheme.typography.labelMedium,
            color = colors.ink2,
        )
    }
}

/**
 * One received file: its extension in mono, its name, its size and its day.
 * Tap opens it in the app that owns its type; long-press offers share and
 * delete, because those two are worth doing but not worth a visible affordance.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ReceivedRow(file: ReceivedFile, onActions: (ReceivedFile) -> Unit) {
    val colors = FlitPalette.colors
    val context = LocalContext.current

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .combinedClickable(
                onClickLabel = "Open ${file.name}",
                onClick = { if (!file.folder) openReceived(context, resolveReceived(context, file.name)) },
                onLongClickLabel = "More actions",
                onLongClick = { onActions(file) },
            )
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = if (file.folder) {
                "DIR"
            } else {
                file.name.substringAfterLast('.', "").uppercase().ifEmpty { "FILE" }
            },
            fontFamily = FontFamily.Monospace,
            fontSize = 10.5.sp,
            letterSpacing = 0.6.sp,
            color = colors.ink3,
            modifier = Modifier.width(52.dp),
        )
        Spacer(Modifier.width(6.dp))
        Text(
            text = file.name,
            style = MaterialTheme.typography.titleSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        Spacer(Modifier.width(12.dp))
        Text(
            text = Formatter.formatShortFileSize(context, file.bytes.toLong()) +
                " \u00b7 " + whenChanged(file.modified.toLong()),
            style = MaterialTheme.typography.bodySmall,
            color = colors.ink3,
        )
    }
}

@Composable
private fun DayDivider(label: String) {
    val colors = FlitPalette.colors

    Text(
        text = label,
        style = MaterialTheme.typography.labelSmall,
        color = colors.ink3,
        modifier = Modifier.padding(top = 14.dp, bottom = 4.dp, start = 2.dp),
    )
}

/**
 * The ink slab that appears once a destination is chosen: it names the target
 * in the display face and offers the one action there is.
 */
@Composable
private fun SendSlab(
    target: String,
    typed: Boolean,
    enabled: Boolean,
    onPick: () -> Unit,
    onClear: () -> Unit,
) {
    val colors = FlitPalette.colors
    val slab = MaterialTheme.colorScheme.onBackground

    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, bottom = 12.dp),
        color = slab,
        shape = RoundedCornerShape(24.dp),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(start = 22.dp, end = 8.dp, top = 12.dp, bottom = 12.dp)
                .heightIn(min = 64.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    text = if (typed) "To address" else "Sending to",
                    style = MaterialTheme.typography.labelSmall,
                    color = colors.paper.copy(alpha = 0.55f),
                )
                Text(
                    text = target,
                    style = MaterialTheme.typography.titleLarge,
                    color = colors.paper,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Button(
                onClick = onPick,
                enabled = enabled,
                shape = RoundedCornerShape(16.dp),
                colors = ButtonDefaults.buttonColors(
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                ),
                contentPadding = PaddingValues(horizontal = 20.dp, vertical = 12.dp),
            ) {
                Text("Choose files", style = MaterialTheme.typography.labelMedium)
            }
            IconTap(label = "Choose another device", onClick = onClear) {
                CloseGlyph(tint = colors.paper.copy(alpha = 0.6f))
            }
        }
    }
}

// --------------------------------------------------------------- Settings --

/**
 * The label, the proof, the address, and what the receiver is doing. All of
 * it readable, one tap to change the name — and the honest sentence about
 * what a name is worth.
 */
@Composable
private fun SettingsTab(
    state: HomeState,
    onRename: () -> Unit,
    onSetAutoAccept: (Boolean) -> Unit,
    onForgetTrusted: (String) -> Unit,
) {
    val colors = FlitPalette.colors
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val device = state.device

    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(start = 20.dp, end = 20.dp, bottom = 24.dp),
    ) {
        item(key = "identity") {
            SectionHeading(title = "Identity", trailing = null)
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(min = 56.dp)
                    .clickable(onClickLabel = "Change this phone's name", onClick = onRename),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = "Name",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.ink3,
                    )
                    Spacer(Modifier.height(2.dp))
                    Text(
                        text = device?.alias ?: "Starting\u2026",
                        style = MaterialTheme.typography.titleLarge,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Chevron(tint = colors.ink3, direction = ChevronDirection.Right)
            }
        }

        item(key = "fingerprint") {
            Spacer(Modifier.height(8.dp))
            Surface(color = colors.raised, shape = RoundedCornerShape(18.dp)) {
                Column(modifier = Modifier
                    .fillMaxWidth()
                    .padding(16.dp)) {
                    Text(
                        text = "Fingerprint",
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.ink3,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text = groupedFingerprint(device?.fingerprint.orEmpty()),
                        fontFamily = FontFamily.Monospace,
                        fontSize = 11.5.sp,
                        lineHeight = 18.sp,
                        letterSpacing = 0.2.sp,
                    )
                    Spacer(Modifier.height(6.dp))
                    Text(
                        text = "Other devices pin this, not the name.",
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.ink3,
                    )
                }
            }
        }

        item(key = "address") {
            Spacer(Modifier.height(10.dp))
            Surface(color = colors.raised, shape = RoundedCornerShape(18.dp)) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 16.dp, end = 4.dp, top = 10.dp, bottom = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            text = "Address",
                            style = MaterialTheme.typography.labelSmall,
                            color = colors.ink3,
                        )
                        Text(
                            text = device?.addr.orEmpty(),
                            fontFamily = FontFamily.Monospace,
                            fontSize = 13.sp,
                        )
                    }
                    TextButton(
                        onClick = {
                            clipboard.setText(AnnotatedString(device?.addr.orEmpty()))
                            Toast.makeText(context, "Address copied", Toast.LENGTH_SHORT).show()
                        },
                    ) {
                        Text("Copy")
                    }
                }
            }
        }

        item(key = "receiver") {
            SectionHeading(title = "Receiver", trailing = null)
            val status = when (device?.discovery) {
                "running" -> "Looking for devices on this Wi\u2011Fi"
                "unavailable" -> "Discovery unavailable; use addresses"
                null -> "Starting\u2026"
                else -> "Not looking for devices"
            }
            Text(
                text = status,
                style = MaterialTheme.typography.bodyMedium,
                color = colors.ink2,
                modifier = Modifier.padding(start = 2.dp),
            )
        }

        item(key = "consent") {
            SectionHeading(title = "Receiving", trailing = null)
            Surface(color = colors.raised, shape = RoundedCornerShape(18.dp)) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 12.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = "Accept files without asking",
                                style = MaterialTheme.typography.titleMedium,
                            )
                            Spacer(Modifier.height(2.dp))
                            Text(
                                text = "Off asks before every send, apart from the devices " +
                                    "you trusted below.",
                                style = MaterialTheme.typography.bodySmall,
                                color = colors.ink3,
                            )
                        }
                        Switch(
                            checked = state.autoAccept,
                            onCheckedChange = onSetAutoAccept,
                        )
                    }
                }
            }
        }

        item(key = "trusted") {
            Spacer(Modifier.height(14.dp))
            Text(
                text = "Trusted devices",
                style = MaterialTheme.typography.titleSmall,
                modifier = Modifier.padding(start = 2.dp),
            )
            Spacer(Modifier.height(2.dp))
            if (state.trusted.isEmpty()) {
                Text(
                    text = "None yet. Tick \u201cAlways accept\u201d on a transfer to add one.",
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink3,
                    modifier = Modifier.padding(start = 2.dp, top = 2.dp),
                )
            } else {
                Surface(color = colors.raised, shape = RoundedCornerShape(18.dp)) {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(start = 16.dp, end = 8.dp, top = 4.dp, bottom = 4.dp),
                    ) {
                        state.trusted.forEach { fingerprint ->
                            // A live peer name is the current one; the saved
                            // name keeps the row legible when that device is
                            // off the network. Neither grants anything.
                            val alias = state.peers
                                .firstOrNull { it.fingerprint.equals(fingerprint, ignoreCase = true) }
                                ?.alias
                                ?.takeIf { it.isNotBlank() }
                                ?: state.trustedNames[fingerprint.lowercase()]
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .heightIn(min = 52.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(modifier = Modifier.weight(1f)) {
                                    Text(
                                        text = alias ?: "Trusted device",
                                        style = MaterialTheme.typography.titleMedium,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                    Text(
                                        text = shortFingerprint(fingerprint),
                                        style = MaterialTheme.typography.bodySmall,
                                        color = colors.ink3,
                                        fontFamily = FontFamily.Monospace,
                                    )
                                }
                                TextButton(onClick = { onForgetTrusted(fingerprint) }) {
                                    Text("Forget")
                                }
                            }
                        }
                    }
                }
            }
        }

        item(key = "about") {
            SectionHeading(title = "About", trailing = null)
            val version = remember {
                runCatching {
                    context.packageManager
                        .getPackageInfo(context.packageName, 0)
                        .versionName
                }.getOrNull()
            }
            Text(
                text = "Flit $version \u00b7 pre-alpha. Files stay on the devices; " +
                    "transfers are encrypted on this Wi-Fi.",
                style = MaterialTheme.typography.bodySmall,
                color = colors.ink3,
                modifier = Modifier.padding(start = 2.dp),
            )
        }
    }
}

// ------------------------------------------------------------- bottom bar --

@Composable
private fun BottomBar(active: FlitTab, onSelect: (FlitTab) -> Unit) {
    val colors = FlitPalette.colors
    val line = MaterialTheme.colorScheme.outline

    Column(modifier = Modifier.navigationBarsPadding()) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .height(1.dp)
                .background(line),
        )
        Row(modifier = Modifier.height(68.dp)) {
            FlitTab.entries.forEach { item ->
                val selected = item == active
                val tint by animateColorAsState(
                    targetValue = if (selected) {
                        MaterialTheme.colorScheme.onBackground
                    } else {
                        colors.ink3
                    },
                    animationSpec = tween(160),
                    label = "tab-${item.name}",
                )

                Column(
                    modifier = Modifier
                        .weight(1f)
                        .fillMaxSize()
                        .clickable(
                            onClickLabel = item.label,
                            onClick = { onSelect(item) },
                        ),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    when (item) {
                        FlitTab.Receive -> RadarGlyph(tint = tint)
                        FlitTab.Send -> SendGlyph(tint = tint)
                        FlitTab.Settings -> SlidersGlyph(tint = tint)
                    }
                    Spacer(Modifier.height(4.dp))
                    Text(
                        text = item.label,
                        style = MaterialTheme.typography.labelSmall,
                        color = tint,
                    )
                }
            }
        }
    }
}

// -------------------------------------------------------------- dialogs --

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
        shape = RoundedCornerShape(24.dp),
        containerColor = FlitPalette.colors.card,
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

/**
 * What can be done with a received file. The name is the title; the actions
 * are two quiet rows, and the destructive one wears the error colour.
 */
@Composable
private fun FileActionsDialog(
    file: ReceivedFile,
    onDismiss: () -> Unit,
    onShare: () -> Unit,
    onDelete: () -> Unit,
) {
    val colors = FlitPalette.colors
    val context = LocalContext.current
    val size = Formatter.formatShortFileSize(context, file.bytes.toLong())

    AlertDialog(
        onDismissRequest = onDismiss,
        shape = RoundedCornerShape(24.dp),
        containerColor = colors.card,
        title = {
            Text(
                text = file.name,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                softWrap = false,
                overflow = TextOverflow.Ellipsis,
            )
        },
        text = {
            Column {
                Text(
                    text = "$size · received " + whenChanged(file.modified.toLong()),
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink3,
                )
                Spacer(Modifier.height(10.dp))
                if (!file.folder) {
                    TextButton(
                        onClick = onShare,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("Share", modifier = Modifier.fillMaxWidth())
                    }
                }
                TextButton(
                    onClick = onDelete,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        text = if (file.folder) "Delete folder" else "Delete",
                        color = MaterialTheme.colorScheme.error,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        },
        confirmButton = {},
        dismissButton = {},
    )
}

/** The way to reach a device whose announcement never arrived. */
@Composable
private fun AddressDialog(
    onDismiss: () -> Unit,
    onUse: (String, String) -> Unit,
) {
    var addr by remember { mutableStateOf("") }
    var fingerprint by remember { mutableStateOf("") }
    val cleanAddr = addr.trim()
    val cleanFingerprint = fingerprint.trim()

    AlertDialog(
        onDismissRequest = onDismiss,
        shape = RoundedCornerShape(24.dp),
        containerColor = FlitPalette.colors.card,
        title = { Text("Send to an address") },
        text = {
            Column {
                Text(
                    text = "For a device on another network, or one whose announcement " +
                        "did not arrive. The fingerprint is still what proves who answers.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = FlitPalette.colors.ink2,
                )
                Spacer(Modifier.height(14.dp))
                OutlinedTextField(
                    value = addr,
                    onValueChange = { addr = it },
                    singleLine = true,
                    label = { Text("Address") },
                    placeholder = { Text("192.168.1.20:47821") },
                )
                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = fingerprint,
                    onValueChange = { fingerprint = it },
                    singleLine = true,
                    label = { Text("Fingerprint") },
                    placeholder = { Text("64 hex characters") },
                    textStyle = MaterialTheme.typography.bodySmall.copy(
                        fontFamily = FontFamily.Monospace,
                    ),
                )
            }
        },
        confirmButton = {
            TextButton(
                enabled = cleanAddr.isNotEmpty() && cleanFingerprint.isNotEmpty(),
                onClick = { onUse(cleanAddr, cleanFingerprint) },
            ) {
                Text("Use")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

/**
 * The question consent exists for: who is asking, what would land, and how much
 * room it needs. Accept sends the ids back to the engine; decline answers empty.
 *
 * The sender is already blocked waiting, so there is no Cancel — closing the
 * sheet is a decline, which is the honest reading of "not now".
 */
@Composable
private fun OfferSheet(
    offer: OfferEvent,
    onAccept: (List<String>, Boolean) -> Unit,
    onDecline: () -> Unit,
) {
    val colors = FlitPalette.colors
    val context = LocalContext.current
    var remember by remember(offer.id) { mutableStateOf(false) }
    val canTrust = offer.fingerprint.isNotEmpty()
    val size = Formatter.formatShortFileSize(context, offer.bytes.toLong())
    val shown = offer.items.take(4)
    val extra = offer.items.size - shown.size
    // The receiver writes under filesDir/received, so this is the number that
    // decides whether the transfer can finish, not the phone's total storage.
    val available = remember(offer.id) {
        runCatching {
            val dir = File(context.filesDir, "received")
            StatFs(dir.path).availableBytes
        }.getOrDefault(Long.MAX_VALUE)
    }
    val fits = offer.bytes.toLong() <= available

    AlertDialog(
        onDismissRequest = onDecline,
        shape = RoundedCornerShape(24.dp),
        containerColor = colors.card,
        title = {
            // Who is asking, in one line, with the size of the ask under it.
            // The alias is the only thing the person can recognise, so it gets
            // the line to itself instead of sharing it with a verb.
            Column {
                Text(
                    text = offer.alias,
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = "wants to send " +
                        (if (offer.items.size == 1) "1 item" else "${offer.items.size} items") +
                        " \u00b7 " + size,
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink3,
                )
            }
        },
        text = {
            Column {
                shown.forEach { item ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 3.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text(
                                text = item.name,
                                style = MaterialTheme.typography.titleSmall,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            // A single file's size is already in the title; only
                            // a list needs it per row.
                            if (offer.items.size > 1 || item.kind == "text") {
                                Text(
                                    text = (if (item.kind == "text") "Text" else
                                        Formatter.formatShortFileSize(
                                            context,
                                            item.size.toLong(),
                                        )),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.ink3,
                                )
                            }
                        }
                    }
                }
                if (extra > 0) {
                    Text(
                        text = "and $extra more",
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.ink3,
                    )
                }
                Spacer(Modifier.height(12.dp))
                if (canTrust) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            text = "from",
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.ink3,
                        )
                        Spacer(Modifier.width(6.dp))
                        Text(
                            text = shortFingerprint(offer.fingerprint),
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.ink2,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                } else {
                    Text(
                        text = "This sender did not prove an identity, so this phone " +
                            "cannot remember it.",
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.ink3,
                    )
                }
                if (canTrust) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { remember = !remember }
                            .padding(vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Checkbox(
                            checked = remember,
                            onCheckedChange = { remember = it },
                        )
                        Spacer(Modifier.width(4.dp))
                        Text(
                            text = "Always accept from this device",
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
                if (!fits) {
                    Spacer(Modifier.height(2.dp))
                    Text(
                        text = "Not enough space on this phone for $size.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        },
        confirmButton = {
            // Accept is the one thing the sheet is for, so it is the only
            // filled control on it. Decline is a plain word, not a rival.
            Button(
                enabled = fits && offer.items.isNotEmpty(),
                shape = RoundedCornerShape(16.dp),
                colors = ButtonDefaults.buttonColors(
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                ),
                contentPadding = PaddingValues(horizontal = 20.dp, vertical = 12.dp),
                onClick = { onAccept(offer.items.map { it.id }, remember) },
            ) {
                Text("Accept", style = MaterialTheme.typography.labelMedium)
            }
        },
        dismissButton = {
            TextButton(onClick = onDecline) {
                Text("Decline", color = colors.ink2)
            }
        },
    )
}

// ------------------------------------------------------------ small parts --

@Composable
private fun ErrorPanel(message: String, onRetry: () -> Unit) {
    val colors = FlitPalette.colors

    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 18.dp),
        color = colors.dangerSoft,
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(modifier = Modifier.padding(start = 20.dp, end = 8.dp, top = 14.dp, bottom = 8.dp)) {
            Text(
                text = "The receiver did not start",
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.error,
            )
            Spacer(Modifier.height(4.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = message,
                    style = MaterialTheme.typography.bodySmall,
                    color = colors.ink2,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = onRetry) { Text("Retry") }
            }
        }
    }
}

@Composable
private fun StartingRow() {
    val colors = FlitPalette.colors

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 24.dp, start = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CircularProgressIndicator(
            modifier = Modifier.size(16.dp),
            strokeWidth = 1.8.dp,
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

/** The first 8 hex characters, grouped as a person reads them off a screen. */
internal fun shortFingerprint(fingerprint: String): String =
    fingerprint.take(8).chunked(4).joinToString(" ")

/** The whole fingerprint, in groups, for where it is checked in full. */
internal fun groupedFingerprint(fingerprint: String): String =
    fingerprint.chunked(4).joinToString(" ")

/** "14:32" today, "16 Nov" before that: a time only means something with a day. */
private fun whenChanged(epochSeconds: Long): String {
    if (epochSeconds <= 0L) return "just now"
    val zone = ZoneId.systemDefault()
    val moment = Instant.ofEpochSecond(epochSeconds).atZone(zone)
    return if (moment.toLocalDate() == LocalDate.now(zone)) {
        moment.format(DateTimeFormatter.ofPattern("HH:mm"))
    } else {
        moment.format(DateTimeFormatter.ofPattern("d MMM"))
    }
}

@Composable
private fun Dot(color: Color) {
    Box(
        modifier = Modifier
            .padding(horizontal = 8.dp)
            .size(3.dp)
            .background(color, CircleShape),
    )
}

// Glyphs. Each is a dozen lines of Canvas: the app needs a handful of marks
// and an icon artifact would add a dependency and a megabyte for the result.

private enum class ChevronDirection { Down, Right }

@Composable
private fun DeviceGlyph(kind: String, tint: Color) {
    Canvas(Modifier.size(26.dp)) {
        val stroke = 1.6.dp.toPx()
        val w = size.width
        val h = size.height

        if (kind == "phone") {
            drawRoundRect(
                color = tint,
                topLeft = Offset(w * 0.30f, h * 0.03f),
                size = Size(w * 0.40f, h * 0.94f),
                cornerRadius = CornerRadius(w * 0.11f),
                style = Stroke(stroke),
            )
            drawCircle(
                color = tint,
                radius = stroke * 0.85f,
                center = Offset(w * 0.50f, h * 0.85f),
            )
        } else {
            // A laptop: a screen with a base, and the base is the line that
            // makes it read as one rather than as a monitor.
            drawRoundRect(
                color = tint,
                topLeft = Offset(w * 0.12f, h * 0.10f),
                size = Size(w * 0.76f, h * 0.58f),
                cornerRadius = CornerRadius(w * 0.06f),
                style = Stroke(stroke),
            )
            drawLine(
                color = tint,
                start = Offset(w * 0.02f, h * 0.82f),
                end = Offset(w * 0.98f, h * 0.82f),
                strokeWidth = stroke,
                cap = StrokeCap.Round,
            )
        }
    }
}

@Composable
private fun Chevron(tint: Color, direction: ChevronDirection) {
    Canvas(
        when (direction) {
            ChevronDirection.Down -> Modifier.size(width = 12.dp, height = 7.dp)
            ChevronDirection.Right -> Modifier.size(width = 7.dp, height = 12.dp)
        },
    ) {
        val stroke = 1.6.dp.toPx()
        val (start, mid, end) = when (direction) {
            ChevronDirection.Down -> Triple(
                Offset(0f, 0f),
                Offset(size.width / 2f, size.height),
                Offset(size.width, 0f),
            )
            ChevronDirection.Right -> Triple(
                Offset(0f, 0f),
                Offset(size.width, size.height / 2f),
                Offset(0f, size.height),
            )
        }
        drawLine(color = tint, start = start, end = mid, strokeWidth = stroke, cap = StrokeCap.Round)
        drawLine(color = tint, start = mid, end = end, strokeWidth = stroke, cap = StrokeCap.Round)
    }
}

@Composable
private fun CloseGlyph(tint: Color) {
    Canvas(Modifier.size(16.dp)) {
        val stroke = 1.7.dp.toPx()
        drawLine(
            color = tint,
            start = Offset(size.width * 0.2f, size.height * 0.2f),
            end = Offset(size.width * 0.8f, size.height * 0.8f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
        drawLine(
            color = tint,
            start = Offset(size.width * 0.8f, size.height * 0.2f),
            end = Offset(size.width * 0.2f, size.height * 0.8f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
    }
}

@Composable
private fun RefreshGlyph(tint: Color) {
    Canvas(Modifier.size(18.dp)) {
        val stroke = 1.6.dp.toPx()
        val inset = stroke
        drawArc(
            color = tint,
            startAngle = -55f,
            sweepAngle = 285f,
            useCenter = false,
            topLeft = Offset(inset, inset),
            size = Size(size.width - inset * 2, size.height - inset * 2),
            style = Stroke(width = stroke, cap = StrokeCap.Round),
        )
        // The arrowhead sits on the open end of the arc, pointing along it.
        val tip = Offset(size.width * 0.86f, size.height * 0.14f)
        drawLine(
            color = tint,
            start = Offset(tip.x - size.width * 0.20f, tip.y),
            end = tip,
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
        drawLine(
            color = tint,
            start = tip,
            end = Offset(tip.x, tip.y + size.height * 0.20f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
    }
}

/** Wi‑Fi arcs over a dot: the receiver, listening. */
@Composable
private fun RadarGlyph(tint: Color) {
    Canvas(Modifier.size(22.dp)) {
        val stroke = 1.7.dp.toPx()
        val cx = size.width / 2f
        val cy = size.height * 0.78f

        for (index in 0..2) {
            val radius = size.width * (0.16f + index * 0.24f)
            drawArc(
                color = tint,
                startAngle = -135f,
                sweepAngle = 90f,
                useCenter = false,
                topLeft = Offset(cx - radius, cy - radius),
                size = Size(radius * 2, radius * 2),
                style = Stroke(width = stroke, cap = StrokeCap.Round),
            )
        }
        drawCircle(color = tint, radius = stroke * 1.1f, center = Offset(cx, cy))
    }
}

/** The paper-plane arrow: a shaft and two barbs. */
@Composable
private fun SendGlyph(tint: Color) {
    Canvas(Modifier.size(20.dp)) {
        val stroke = 1.7.dp.toPx()
        drawLine(
            color = tint,
            start = Offset(size.width * 0.10f, size.height * 0.90f),
            end = Offset(size.width * 0.88f, size.height * 0.12f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
        drawLine(
            color = tint,
            start = Offset(size.width * 0.88f, size.height * 0.12f),
            end = Offset(size.width * 0.88f, size.height * 0.58f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
        drawLine(
            color = tint,
            start = Offset(size.width * 0.88f, size.height * 0.12f),
            end = Offset(size.width * 0.42f, size.height * 0.12f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
    }
}

/** Three tracks with knobs: settings without a gear. */
@Composable
private fun SlidersGlyph(tint: Color) {
    Canvas(Modifier.size(20.dp)) {
        val stroke = 1.7.dp.toPx()
        val knobs = listOf(0.62f, 0.30f, 0.52f)

        for (index in 0..2) {
            val y = size.height * (0.18f + index * 0.32f)
            drawLine(
                color = tint,
                start = Offset(size.width * 0.06f, y),
                end = Offset(size.width * 0.94f, y),
                strokeWidth = stroke,
                cap = StrokeCap.Round,
            )
            drawCircle(
                color = tint,
                radius = stroke * 1.6f,
                center = Offset(size.width * knobs[index], y),
            )
        }
    }
}

/**
 * Two machines and a path between them: the near phone in the accent because
 * this phone is the near one, the far screen grey because it has not arrived.
 */
@Composable
private fun EmptyGlyph(near: Color, far: Color) {
    Canvas(Modifier.size(width = 120.dp, height = 64.dp)) {
        val stroke = 1.5.dp.toPx()

        // The near side: a phone.
        drawRoundRect(
            color = near,
            topLeft = Offset(size.width * 0.10f, size.height * 0.08f),
            size = Size(size.width * 0.18f, size.height * 0.74f),
            cornerRadius = CornerRadius(size.width * 0.035f),
            style = Stroke(stroke),
        )

        // The far side: a laptop, screen and base.
        drawRoundRect(
            color = far,
            topLeft = Offset(size.width * 0.70f, size.height * 0.10f),
            size = Size(size.width * 0.24f, size.height * 0.48f),
            cornerRadius = CornerRadius(size.width * 0.025f),
            style = Stroke(stroke),
        )
        drawLine(
            color = far,
            start = Offset(size.width * 0.67f, size.height * 0.70f),
            end = Offset(size.width * 0.97f, size.height * 0.70f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )

        // Three dots, not a line: nothing is connected yet.
        for (index in 0..2) {
            drawCircle(
                color = far.copy(alpha = 0.65f),
                radius = stroke * 0.85f,
                center = Offset(
                    size.width * (0.36f + index * 0.10f),
                    size.height * 0.42f,
                ),
            )
        }
    }
}
