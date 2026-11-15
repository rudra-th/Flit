package dev.flit.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel

/**
 * One activity, one screen. The engine lives in [HomeViewModel], which is why
 * rotating the phone does not tear down the receiver and re-announce as a new
 * peer for two seconds.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // The window draws behind the status and navigation bars; the screen
        // pads itself with the safe area instead of guessing the heights.
        enableEdgeToEdge()

        setContent {
            FlitTheme {
                val home: HomeViewModel = viewModel()
                val state by home.state.collectAsStateWithLifecycle()

                HomeScreen(state = state, onRefresh = home::refresh)
            }
        }
    }
}
