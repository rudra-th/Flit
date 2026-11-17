package dev.flit.android

import android.app.Application
import android.content.Context
import android.net.Uri
import android.net.wifi.WifiManager
import android.provider.OpenableColumns
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.flit.android.ffi.DeviceInfo
import dev.flit.android.ffi.MobileEngine
import dev.flit.android.ffi.OfferEvent
import dev.flit.android.ffi.OfferListener
import dev.flit.android.ffi.PeerInfo
import dev.flit.android.ffi.ReceivedFile
import dev.flit.android.ffi.TransferEvent
import dev.flit.android.ffi.TransferListener
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** What the screen draws. One object, one recomposition per refresh. */
data class HomeState(
    val device: DeviceInfo? = null,
    val peers: List<PeerInfo> = emptyList(),
    /** Device id chosen as the destination, if any. */
    val selected: String? = null,
    /** A typed address chosen as the destination instead of a peer. */
    val manual: ManualTarget? = null,
    /** The send the row above the list is about, if one is recent. */
    val transfer: TransferUi? = null,
    /** What has landed in the receive folder. */
    val received: List<ReceivedFile> = emptyList(),
    /** A row removed for undo; the disk delete waits until the snackbar goes. */
    val removed: ReceivedFile? = null,
    /** A transfer offered right now, waiting for an answer. */
    val offer: OfferEvent? = null,
    /** Accept every offer without asking. Persisted. */
    val autoAccept: Boolean = false,
    /** Fingerprints accepted on their own. Persisted. */
    val trusted: List<String> = emptyList(),
    val error: String? = null,
    /** One sentence to show once, then clear. */
    val notice: String? = null,
    val refreshing: Boolean = false,
)

/** A destination the person typed: an address and the key it must prove. */
data class ManualTarget(
    val addr: String,
    val fingerprint: String,
)

/** One send, flattened for the row that draws it. */
data class TransferUi(
    val id: ULong,
    val label: String,
    val sent: Long,
    val total: Long,
    val filesDone: Int,
    val files: Int,
    val finished: Boolean = false,
    val cancelled: Boolean = false,
    /** Why it failed, or which files were refused. */
    val problem: String? = null,
) {
    /** No more events will arrive for this send. */
    val done: Boolean get() = finished || cancelled || problem != null

    /** 0f..1f; 0f while the total is not known yet. */
    val fraction: Float
        get() = when {
            total <= 0L -> 0f
            sent >= total -> 1f
            else -> (sent.toDouble() / total.toDouble()).toFloat()
        }
}

/**
 * Owns the engine for as long as the screen exists.
 *
 * The engine is a Rust object: it holds a socket, a discovery listener and a
 * runtime. Nothing in this class talks to the protocol; it starts the object,
 * asks it who is nearby, and stops it when the process is done with it.
 */
class HomeViewModel(app: Application) : AndroidViewModel(app) {
    private val _state = MutableStateFlow(HomeState())
    val state: StateFlow<HomeState> = _state.asStateFlow()

    private var engine: MobileEngine? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var poller: Job? = null
    /** Runs out the undo window; cancelled by undo, commits the disk delete. */
    private var deleteJob: Job? = null
    private var starting = false
    private lateinit var mediaImporter: MediaImporter

    init {
        start()
    }

    /** Start receiving and announcing, once. */
    fun start() {
        if (starting || engine != null) return
        starting = true

        viewModelScope.launch {
            val app = getApplication<Application>()
            // Read the saved policy before the engine binds: the receiver
            // announces the moment it starts, and an offer must be answered by
            // the policy already in hand, not by one pushed a second later.
            val (autoAccept, trusted) = withContext(Dispatchers.IO) { loadPolicy() }
            val started = withContext(Dispatchers.IO) {
                runCatching {
                    // Multicast is filtered by the Wi-Fi stack unless a lock is
                    // held: without it the phone announces itself but never
                    // hears anyone, which looks like a broken app.
                    multicast = acquireMulticast(app)
                    MobileEngine.start(app.filesDir.absolutePath, autoAccept, trusted)
                }
            }

            started.onSuccess { open ->
                engine = open
                _state.update {
                    it.copy(
                        error = null,
                        autoAccept = autoAccept,
                        trusted = trusted,
                    )
                }
                listen(open)
                listenOffers(open)
                poll()
                refreshReceived()
            }.onFailure { err ->
                _state.update { it.copy(error = describe(err)) }
            }
        }
    }

    /** Read the engine once and publish it. Also called on a timer. */
    fun refresh() {
        val open = engine ?: return
        if (_state.value.refreshing) return
        _state.update { it.copy(refreshing = true) }

        viewModelScope.launch {
            val read = withContext(Dispatchers.IO) {
                runCatching { open.device() to open.peers() }
            }
            read.onSuccess { (device, peers) ->
                _state.update { it.copy(device = device, peers = peers, error = null, refreshing = false) }
            }.onFailure { err ->
                _state.update { it.copy(error = describe(err), refreshing = false) }
            }
        }
    }

    /** Read the engine and the receive folder once: what the header offers. */
    fun refreshAll() {
        refresh()
        refreshReceived()
    }

    private fun poll() {
        poller?.cancel()
        poller = viewModelScope.launch {
            while (isActive) {
                refresh()
                // A file can land while the screen is open — an incoming send
                // has no event of its own — so the ledger rides along.
                refreshReceived()
                delay(POLL_MS)
            }
        }
    }

    /**
     * Attach the one listener this engine will have.
     *
     * Events arrive on a Rust thread. `MutableStateFlow.update` is safe there,
     * so nothing is posted back to the main loop just to store a number; the
     * receive folder is re-read when one of this phone's own sends ends, and
     * again whenever the screen comes back, because an incoming send has no
     * event of its own.
     */
    private fun listen(open: MobileEngine) {
        val app = getApplication<Application>()

        open.watch(object : TransferListener {
            override fun onEvent(event: TransferEvent) {
                // The row is cheap to update here; the receive folder is read
                // once, when a send ends, because that is when it changes.
                val landed = if (event.kind == "finished") {
                    runCatching { open.received() }.getOrDefault(emptyList())
                } else {
                    null
                }

                _state.update { current ->
                    current.copy(
                        transfer = merge(current.transfer, event),
                        received = landed ?: current.received,
                    )
                }

                when (event.kind) {
                    "started" -> TransferService.started(app, event.label)
                    "progress" -> TransferService.progress(
                        app,
                        event.sent.toLong(),
                        event.bytes.toLong(),
                    )
                    "finished", "failed", "cancelled" -> TransferService.finished(app)
                }
            }
        })
    }

    /**
     * Attach the offer listener: who is asking, and when the question closes.
     *
     * A second subscription, separate from [listen], because progress ticks and
     * consent questions are different things: one is drawn, the other is
     * answered. The engine closes an unanswered offer itself, so this listener
     * only has to take the sheet down.
     */
    private fun listenOffers(open: MobileEngine) {
        open.watchOffers(object : OfferListener {
            override fun onOffer(offer: OfferEvent) {
                _state.update { it.copy(offer = offer) }
            }

            override fun onOfferClosed(id: ULong, outcome: String) {
                _state.update { current ->
                    if (current.offer?.id == id) current.copy(offer = null) else current
                }
                if (outcome == "timed_out") {
                    _state.update { it.copy(notice = "That offer timed out.") }
                }
            }
        })
    }

    /** Turn one event into the row's next value. */
    private fun merge(current: TransferUi?, event: TransferEvent): TransferUi {
        val base = current?.takeIf { it.id == event.id } ?: TransferUi(
            id = event.id,
            label = event.label,
            sent = 0,
            total = 0,
            filesDone = 0,
            files = 0,
        )

        return when (event.kind) {
            "started" -> base.copy(
                label = event.label.ifEmpty { base.label },
                total = event.bytes.toLong(),
                files = event.files.toInt(),
            )
            "progress" -> base.copy(
                sent = event.sent.toLong(),
                total = event.bytes.toLong(),
                filesDone = event.filesDone.toInt(),
                files = if (event.files.toInt() > 0) event.files.toInt() else base.files,
            )
            "finished" -> base.copy(
                sent = event.sent.toLong(),
                total = if (event.bytes.toLong() > 0) event.bytes.toLong() else base.total,
                filesDone = event.filesDone.toInt(),
                files = event.files.toInt(),
                finished = true,
                problem = event.detail.ifEmpty { null },
            )
            "failed" -> base.copy(problem = event.detail.ifEmpty { "The send stopped." })
            "cancelled" -> base.copy(cancelled = true)
            else -> base
        }
    }

    /**
     * Choose a destination. Tapping a device always selects it: the tap already
     * opens the picker, so toggling would leave a device chosen with no next
     * step. [clearTarget] is the way to choose nothing.
     */
    fun choose(deviceId: String) {
        _state.update {
            it.copy(selected = deviceId, manual = null)
        }
    }

    /** Use a typed address instead of a discovered device. */
    fun useAddress(addr: String, fingerprint: String) {
        _state.update {
            it.copy(selected = null, manual = ManualTarget(addr, fingerprint))
        }
    }

    /** Forget the destination, whichever kind it was. */
    fun clearTarget() {
        _state.update { it.copy(selected = null, manual = null) }
    }

    /**
     * Send the files behind `uris` to the chosen destination.
     *
     * A picked file is a `content://` URI the engine cannot open, so each one
     * is copied into the cache first. That costs a copy, which is honest and
     * simple; streaming straight from the descriptor needs a send path that
     * takes file descriptors, and that is not this week.
     */
    fun send(uris: List<Uri>) {
        val open = engine ?: return
        val target = _state.value.selected
        val manual = _state.value.manual
        if (target == null && manual == null) {
            _state.update { it.copy(notice = "Choose a device first.") }
            return
        }

        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val outbox = File(getApplication<Application>().cacheDir, "outbox").apply { mkdirs() }
                    uris.mapNotNull { copyIn(outbox, it) }.map { it.absolutePath }
                }.mapCatching { paths ->
                    if (paths.isEmpty()) error("No file could be read.")
                    when {
                        target != null -> open.sendFiles(target, paths)
                        manual != null -> open.sendFilesTo(manual.addr, manual.fingerprint, paths)
                        else -> error("Choose a device first.")
                    }
                }
            }

            result.onFailure { err ->
                _state.update { it.copy(notice = describe(err)) }
            }
        }
    }

    /** Stop the send the row is showing. */
    fun cancelSend() {
        val open = engine ?: return
        val id = _state.value.transfer?.id ?: return
        viewModelScope.launch {
            withContext(Dispatchers.IO) { runCatching { open.cancel(id) } }
        }
    }

    /** A finished row has been read; clear it. */
    fun dismissTransfer() {
        _state.update { it.copy(transfer = null) }
    }

    /**
     * Answer the offer on screen.
     *
     * `accepted` is the list of item ids that may be saved; empty declines the
     * whole transfer. `remember` trusts the sender's fingerprint, which only
     * works when it presented a certificate: a name is never remembered.
     */
    fun answerOffer(accepted: List<String>, remember: Boolean) {
        val open = engine ?: return
        val offer = _state.value.offer ?: return
        val fingerprint = offer.fingerprint
        val trusting = remember && accepted.isNotEmpty() && fingerprint.isNotEmpty()
        // Take the sheet down now: the sender should not wait for a round trip
        // to the engine while the person is still looking at a dead question.
        _state.update { current ->
            if (current.offer?.id == offer.id) current.copy(offer = null) else current
        }

        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val answered = open.answerOffer(offer.id, accepted, trusting)
                    val trusted = if (answered && trusting) {
                        open.trustedDevices().also { savePolicy(_state.value.autoAccept, it) }
                    } else {
                        null
                    }
                    answered to trusted
                }
            }

            result.onSuccess { (answered, trusted) ->
                when {
                    !answered -> _state.update {
                        it.copy(notice = "That offer already closed.")
                    }
                    trusted != null -> _state.update { it.copy(trusted = trusted) }
                }
            }.onFailure { err ->
                _state.update { it.copy(notice = describe(err)) }
            }
        }
    }

    /** Accept every offer without asking, or stop doing that. Persisted. */
    fun setAutoAccept(enabled: Boolean) {
        val open = engine
        val trusted = _state.value.trusted
        _state.update { it.copy(autoAccept = enabled) }
        viewModelScope.launch {
            withContext(Dispatchers.IO) {
                runCatching {
                    open?.setAutoAccept(enabled, trusted)
                    savePolicy(enabled, trusted)
                }
            }
        }
    }

    /** Stop accepting a device on its own. The next transfer asks again. */
    fun forgetTrusted(fingerprint: String) {
        val open = engine
        val autoAccept = _state.value.autoAccept
        val trusted = _state.value.trusted.filterNot { it == fingerprint }
        _state.update { it.copy(trusted = trusted) }
        viewModelScope.launch {
            withContext(Dispatchers.IO) {
                runCatching {
                    open?.setAutoAccept(autoAccept, trusted)
                    savePolicy(autoAccept, trusted)
                }
            }
        }
    }

    /** The policy saved on this phone. */
    private fun loadPolicy(): Pair<Boolean, List<String>> {
        val store = getApplication<Application>()
            .getSharedPreferences(CONSENT_PREFS, Context.MODE_PRIVATE)
        val enabled = store.getBoolean(KEY_AUTO_ACCEPT, false)
        val trusted = store.getStringSet(KEY_TRUSTED, emptySet()).orEmpty().sorted()
        return enabled to trusted
    }

    /** Keep the policy across a restart; the engine holds only the live copy. */
    private fun savePolicy(enabled: Boolean, trusted: List<String>) {
        getApplication<Application>()
            .getSharedPreferences(CONSENT_PREFS, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_AUTO_ACCEPT, enabled)
            .putStringSet(KEY_TRUSTED, trusted.toSet())
            .apply()
    }

    /** What has landed in the receive folder, newest first. */
    fun refreshReceived() {
        val open = engine ?: return
        viewModelScope.launch {
            val files = withContext(Dispatchers.IO) {
                runCatching { open.received() }.getOrDefault(emptyList())
            }
            if (!::mediaImporter.isInitialized) {
                mediaImporter = MediaImporter(getApplication())
            }
            withContext(Dispatchers.IO) { mediaImporter.importAll(files) }
            // A row the person removed stays removed even when the folder is
            // re-read before the delete commits: it is still on disk.
            _state.update { current ->
                current.copy(
                    received = files.filterNot { it.name == current.removed?.name },
                )
            }
        }
    }

    /**
     * Take a row out of the ledger now; the disk delete happens only when the
     * undo window closes. A second delete commits the first one immediately,
     * because only one snack can be pending.
     */
    fun removeReceived(file: ReceivedFile) {
        commitPendingDelete()
        _state.update { it.copy(removed = file, received = it.received - file) }
        deleteJob = viewModelScope.launch {
            delay(UNDO_MS)
            commitPendingDelete()
        }
    }

    /** The undo tap: nothing was deleted, put the row back where it was. */
    fun undoRemoveReceived() {
        deleteJob?.cancel()
        deleteJob = null
        _state.update { it.copy(removed = null) }
        refreshReceived()
    }

    /** Delete the pending row for real, off the main thread. */
    private fun commitPendingDelete() {
        val file = _state.value.removed ?: return
        deleteJob?.cancel()
        deleteJob = null
        _state.update { it.copy(removed = null, received = it.received - file) }
        viewModelScope.launch {
            withContext(Dispatchers.IO) { deleteReceived(getApplication(), file.name) }
        }
    }

    /** Copy one picked document into the cache under a usable name. */
    private fun copyIn(dir: File, uri: Uri): File? = runCatching {
        val resolver = getApplication<Application>().contentResolver
        val raw = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
        val safe = (raw ?: "file")
            .replace(Regex("[\\\\/:*?\"<>|]"), "_")
            .take(120)
            .ifBlank { "file" }

        var target = File(dir, safe)
        var index = 2
        while (target.exists()) {
            target = File(dir, "$index-$safe")
            index += 1
        }

        resolver.openInputStream(uri)?.use { input ->
            target.outputStream().use { output -> input.copyTo(output) }
        } ?: return null

        target
    }.getOrNull()

    override fun onCleared() {
        poller?.cancel()
        // The engine is about to abort anything in flight, so the foreground
        // service has nothing left to justify it.
        TransferService.finished(getApplication())
        // A row still waiting out its undo window deletes for real now: the
        // snackbar it belonged to dies with the screen.
        _state.value.removed?.let { pending ->
            runCatching { deleteReceived(getApplication(), pending.name) }
        }
        val open = engine
        engine = null
        if (open != null) {
            // stop() closes the socket; destroy() frees the Rust object. Both on
            // a worker thread: the receiver joins its tasks on the way out.
            kotlin.concurrent.thread(name = "flit-stop") {
                runCatching { open.stop() }
                runCatching { open.destroy() }
            }
        }
        multicast?.let { lock ->
            runCatching { if (lock.isHeld) lock.release() }
        }
        multicast = null
        super.onCleared()
    }

    /**
     * Save a new name, then open a fresh receiver.
     *
     * The announce carries the name the engine started with, so a rename only
     * reaches the other screen after a restart. Doing it here keeps that rule
     * out of the UI, which just calls this and closes its dialog.
     */
    fun rename(name: String) {
        val open = engine ?: return
        engine = null
        poller?.cancel()
        TransferService.finished(getApplication())

        viewModelScope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching { open.rename(name) }
                    .also {
                        runCatching { open.stop() }
                        runCatching { open.destroy() }
                    }
            }

            result.onSuccess { saved ->
                _state.update { it.copy(notice = "This phone is now $saved.") }
            }.onFailure { err ->
                _state.update { it.copy(notice = describe(err)) }
            }
            start()
        }
    }

    /** The screen showed the notice; do not show it again. */
    fun clearNotice() {
        _state.update { it.copy(notice = null) }
    }

    private fun acquireMulticast(context: Context): WifiManager.MulticastLock? = try {
        @Suppress("DEPRECATION")
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
        wifi?.createMulticastLock("flit-discovery")?.apply {
            setReferenceCounted(false)
            acquire()
        }
    } catch (err: SecurityException) {
        // Without CHANGE_WIFI_MULTICAST_STATE the phone still sends; it just
        // will not hear the other side. The screen says so through the empty
        // state rather than crashing here.
        null
    }

    /**
     * One readable sentence out of whatever the FFI threw.
     *
     * UniFFI's generated `message` for a field-carrying variant is
     * `message=<text>`, which is a debug rendering, not a sentence.
     */
    private fun describe(err: Throwable): String {
        val raw = err.message
        return raw
            ?.removePrefix("message=")
            ?.takeIf { it.isNotBlank() }
            ?: "The receiver stopped answering."
    }

    private companion object {
        /** Discovery announces every second; two seconds keeps the list fresh. */
        const val POLL_MS = 2_000L

        /** How long the undo offer stays on screen before the delete commits. */
        const val UNDO_MS = 5_000L

        /** Where the consent policy is kept between runs. */
        const val CONSENT_PREFS = "consent"
        const val KEY_AUTO_ACCEPT = "auto_accept"
        const val KEY_TRUSTED = "trusted"
    }
}
