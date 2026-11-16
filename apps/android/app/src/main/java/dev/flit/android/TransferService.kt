package dev.flit.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import androidx.core.content.ContextCompat
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Keeps the process alive while a send is on the wire.
 *
 * Android freezes or kills a background app mid-transfer; a `dataSync`
 * foreground service is the supported way to say "still working". The
 * notification is the price of that, and on a locked screen it is the only
 * place progress is visible.
 *
 * The service is deliberately dumb: it draws a label and a bar. The engine
 * owns the transfer; this only survives the screen going dark.
 */
class TransferService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        running.set(true)

        val channel = NotificationChannel(
            CHANNEL,
            "Transfers",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Progress of a file send"
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                label = intent.getStringExtra(EXTRA_LABEL).orEmpty().ifBlank { "Flit" }
                startForeground(NOTIFICATION_ID, build(label, 0, 0))
            }

            ACTION_PROGRESS -> {
                val sent = intent.getLongExtra(EXTRA_SENT, 0)
                val total = intent.getLongExtra(EXTRA_TOTAL, 0)
                getSystemService(NotificationManager::class.java)
                    .notify(NOTIFICATION_ID, build(label, sent, total))
            }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running.set(false)
        super.onDestroy()
    }

    private fun build(name: String, sent: Long, total: Long): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )

        val builder = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle(name)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)

        if (total > 0L) {
            val percent = ((sent.coerceAtMost(total) * 100) / total).toInt()
            builder.setProgress(100, percent, false)
            builder.setContentText("$percent%")
        } else {
            builder.setProgress(0, 0, true)
            builder.setContentText("Preparing")
        }

        return builder.build()
    }

    companion object {
        private const val CHANNEL = "transfers"
        private const val NOTIFICATION_ID = 41

        private const val ACTION_START = "dev.flit.android.action.START"
        private const val ACTION_PROGRESS = "dev.flit.android.action.PROGRESS"
        private const val EXTRA_LABEL = "label"
        private const val EXTRA_SENT = "sent"
        private const val EXTRA_TOTAL = "total"

        /** Whether this process has a service to talk to. */
        private val running = AtomicBoolean(false)

        /** Last label seen, so progress updates do not have to carry it. */
        private var label = "Flit"

        /** A send began: put the process in the foreground for it. */
        fun started(context: Context, name: String) {
            label = name.ifBlank { "Flit" }
            val intent = Intent(context, TransferService::class.java)
                .setAction(ACTION_START)
                .putExtra(EXTRA_LABEL, label)
            ContextCompat.startForegroundService(context, intent)
        }

        /** Bytes moved. Ignored when no send is being tracked. */
        fun progress(context: Context, sent: Long, total: Long) {
            if (!running.get()) return
            val intent = Intent(context, TransferService::class.java)
                .setAction(ACTION_PROGRESS)
                .putExtra(EXTRA_SENT, sent)
                .putExtra(EXTRA_TOTAL, total)
            runCatching { context.startService(intent) }
        }

        /** Nothing is in flight any more; the notification goes with it. */
        fun finished(context: Context) {
            if (!running.getAndSet(false)) return
            runCatching { context.stopService(Intent(context, TransferService::class.java)) }
        }
    }
}
