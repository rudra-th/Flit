package dev.flit.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle

/**
 * One activity, one screen. The engine lives in [HomeViewModel], which is why
 * rotating the phone does not tear down the receiver and re-announce as a new
 * peer for two seconds.
 */
class MainActivity : ComponentActivity() {
    /** Held by the activity, not the composition, so [onResume] can reach it. */
    private val home: HomeViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // The window draws behind the status and navigation bars; the screen
        // pads itself with the safe area instead of guessing the heights.
        enableEdgeToEdge()

        setContent {
            FlitTheme {
                val state by home.state.collectAsStateWithLifecycle()

                HomeScreen(
                    state = state,
                    onRefresh = home::refreshAll,
                    onRename = home::rename,
                    onNoticeShown = home::clearNotice,
                    onChoose = home::choose,
                    onSend = home::send,
                    onCancelSend = home::cancelSend,
                    onDismissTransfer = home::dismissTransfer,
                    onUseAddress = home::useAddress,
                    onClearTarget = home::clearTarget,
                    onRemoveReceived = home::removeReceived,
                    onUndoRemove = home::undoRemoveReceived,
                )
            }
        }
    }

    /**
     * A file can land while the screen is off or another app is in front, and
     * the engine has no event for a receive. Re-reading the folder here is what
     * makes the list match the disk when someone comes back.
     */
    override fun onResume() {
        super.onResume()
        home.refreshReceived()
    }
}
