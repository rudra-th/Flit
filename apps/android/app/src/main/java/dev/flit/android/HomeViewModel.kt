package dev.flit.android

import android.app.Application
import android.content.Context
import android.net.wifi.WifiManager
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.flit.android.ffi.DeviceInfo
import dev.flit.android.ffi.MobileEngine
import dev.flit.android.ffi.PeerInfo
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
    val error: String? = null,
    /** One sentence to show once, then clear. */
    val notice: String? = null,
    val refreshing: Boolean = false,
)

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
    private var starting = false

    init {
        start()
    }

    /** Start receiving and announcing, once. */
    fun start() {
        if (starting || engine != null) return
        starting = true

        viewModelScope.launch {
            val app = getApplication<Application>()
            val started = withContext(Dispatchers.IO) {
                runCatching {
                    // Multicast is filtered by the Wi-Fi stack unless a lock is
                    // held: without it the phone announces itself but never
                    // hears anyone, which looks like a broken app.
                    multicast = acquireMulticast(app)
                    MobileEngine.start(app.filesDir.absolutePath)
                }
            }

            started.onSuccess { open ->
                engine = open
                _state.update { it.copy(error = null) }
                poll()
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

    private fun poll() {
        poller?.cancel()
        poller = viewModelScope.launch {
            while (isActive) {
                refresh()
                delay(POLL_MS)
            }
        }
    }

    override fun onCleared() {
        poller?.cancel()
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
    }
}
