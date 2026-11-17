package dev.flit.android

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import dev.flit.android.ffi.ReceivedFile
import java.io.File

/**
 * Everything a person can do with a file that landed on this phone: open it,
 * hand it to another app, or throw it away. Every one of these goes through
 * [FileProvider], so nothing here needs a storage permission.
 */

/** Where the engine writes what it receives, as the UI sees it. */
internal fun receivedDir(context: Context): File =
    File(context.filesDir, "received")

/** The file behind a ledger row. Names are relative paths the engine validated. */
internal fun resolveReceived(context: Context, name: String): File =
    File(receivedDir(context), name)

/**
 * The mime a viewer will accept, from the extension. Unknown extensions get
 * the generic octet stream, which is what the system's chooser needs to
 * offer "open with" instead of refusing outright.
 */
internal fun mimeFor(name: String): String {
    val extension = name.substringAfterLast('.', "").lowercase()
    if (extension.isEmpty()) return "application/octet-stream"
    return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
        ?: "application/octet-stream"
}

/** A uri another app is allowed to read, until it is done reading. */
internal fun sharedUri(context: Context, file: File): Uri =
    FileProvider.getUriForFile(context, context.packageName + ".fileprovider", file)

/** Open a received file in whatever app handles its type. */
internal fun openReceived(context: Context, file: File) {
    val uri = sharedUri(context, file)
    val view = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, mimeFor(file.name))
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    runCatching { context.startActivity(view) }
        .onFailure {
            android.widget.Toast.makeText(
                context,
                "Nothing on this phone opens this kind of file.",
                android.widget.Toast.LENGTH_SHORT,
            ).show()
        }
}

/** Hand a received file to another app's share sheet. */
internal fun shareReceived(context: Context, file: File) {
    val uri = sharedUri(context, file)
    val send = Intent(Intent.ACTION_SEND).apply {
        type = mimeFor(file.name)
        putExtra(Intent.EXTRA_STREAM, uri)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    val chooser = Intent.createChooser(send, "Share " + file.name).apply {
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    runCatching { context.startActivity(chooser) }
}

/**
 * Delete from disk. Folders go wholesale; the engine writes only inside the
 * received folder, so nothing outside it can be reached this way.
 */
internal fun deleteReceived(context: Context, name: String) {
    resolveReceived(context, name).deleteRecursively()
}

/**
 * Images and videos a person expects to find in their gallery, not inside a
 * file manager. Each media file is imported once, remembered by name, size
 * and mtime, so a refresh never duplicates a row in the gallery.
 *
 * Own-app writes to MediaStore need no permission on API 29+, which is the
 * floor for this: on older builds the files stay in the received folder and
 * remain openable from the ledger.
 */
internal class MediaImporter(context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("flit-media", Context.MODE_PRIVATE)
    private val imported = prefs.getStringSet("imported", emptySet())?.toMutableSet() ?: mutableSetOf()

    /** Import every not-yet-imported media file in the list. */
    fun importAll(files: List<ReceivedFile>) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        var changed = false
        for (file in files) {
            if (file.folder) continue
            val mime = mimeFor(file.name)
            if (!mime.startsWith("image/") && !mime.startsWith("video/")) continue
            val key = file.name + ":" + file.bytes + ":" + file.modified
            if (imported.contains(key)) continue

            val source = resolveReceived(app, file.name)
            if (!source.isFile) continue
            if (insert(file, mime)) {
                imported += key
                changed = true
            }
        }
        if (changed) {
            prefs.edit().putStringSet("imported", imported).apply()
        }
    }

    private fun insert(file: ReceivedFile, mime: String): Boolean = runCatching {
        val source = resolveReceived(app, file.name)
        val collection = if (mime.startsWith("video/")) {
            MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        } else {
            MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        }
        val relative = if (mime.startsWith("video/")) "Movies/Flit" else "Pictures/Flit"

        val values = android.content.ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, source.name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                put(MediaStore.MediaColumns.RELATIVE_PATH, relative)
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
        }
        val resolver = app.contentResolver
        val uri = resolver.insert(collection, values) ?: return false
        resolver.openOutputStream(uri)?.use { out ->
            source.inputStream().use { it.copyTo(out) }
        } ?: return false
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            values.clear()
            values.put(MediaStore.MediaColumns.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
        }
        true
    }.getOrDefault(false)
}
