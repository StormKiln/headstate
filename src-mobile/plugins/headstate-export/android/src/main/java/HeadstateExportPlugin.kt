package com.pktstorm.headstate.export

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.FileProvider
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.Executors

// A distinct manifest component preserves Tauri's own FileProvider.
class MarkdownExportProvider : FileProvider()

enum class ExportKind(val wire: String, val filename: String, val mime: String) {
    TRANSCRIPT("transcript_markdown", "transcript.md", "text/markdown"),
    MEASUREMENT("measurement_jsonl", "headstate-measurements.jsonl", "application/x-ndjson")
}
class ExportCapacityException : Exception()
class MarkdownExportStore(private val root: File) {
    fun prepare(markdown: String, now: Long = System.currentTimeMillis(), kind: ExportKind = ExportKind.TRANSCRIPT): File {
        val bytes = markdown.toByteArray(Charsets.UTF_8)
        require(bytes.size <= 8 * 1024 * 1024) { "Export exceeds 8 MiB" }
        check(root.isDirectory || root.mkdirs()) { "Export storage unavailable" }
        fun entries() = root.listFiles()?.filter { it.name.matches(Regex("[a-f0-9]{64}")) }
            ?: throw IllegalStateException("Export storage unavailable")
        entries().filter { now - it.lastModified() > 24 * 60 * 60 * 1000L }.forEach {
            check(it.deleteRecursively()) { "Export cleanup unavailable" }
        }
        val hash = MessageDigest.getInstance("SHA-256").digest(kind.wire.toByteArray(Charsets.UTF_8) + byteArrayOf(0) + bytes).joinToString("") { "%02x".format(it) }
        val folder = File(root, hash)
        val file = File(folder, kind.filename)
        val staged = File(folder, ".transcript.pending")
        // Validate older cache entries too: a previous process may have died
        // while writing the final filename before atomic publication existed.
        if (file.isFile && file.length() == bytes.size.toLong() && file.readBytes().contentEquals(bytes)) {
            check(!staged.exists() || staged.delete())
            check(folder.setLastModified(now))
            return file
        }
        val live = entries().filter { it != folder }
        if (live.size >= 8 || live.sumOf { folder -> ExportKind.values().sumOf { File(folder, it.filename).length() } } + bytes.size > 32 * 1024 * 1024) throw ExportCapacityException()
        check(folder.isDirectory || folder.mkdir())
        try {
            // A retry truncates an abandoned stage. Android's same-directory
            // rename publishes complete synced bytes atomically; never stream
            // directly into a filename that can be shared or reused.
            staged.outputStream().use { it.write(bytes); it.fd.sync() }
            check(staged.renameTo(file)) { "Export publication unavailable" }
            check(folder.setLastModified(now))
            return file
        } catch (error: Exception) { staged.delete(); throw error }
    }
}

@InvokeArg
class ShareArgs { lateinit var markdown: String; var kind: String = "transcript_markdown" }
@TauriPlugin
class HeadstateExportPlugin(private val activity: Activity) : Plugin(activity) {
    private val worker = Executors.newSingleThreadExecutor()
    private var pending: Invoke? = null
    @Command
    fun share(invoke: Invoke) {
        val args = invoke.parseArgs(ShareArgs::class.java)
        val kind = ExportKind.values().firstOrNull { it.wire == args.kind }
        if (kind == null) { reply(invoke, "failed"); return }
        activity.runOnUiThread {
            if (pending != null) { reply(invoke, "busy"); return@runOnUiThread }
            if (activity.isFinishing || activity.isDestroyed) { reply(invoke, "failed"); return@runOnUiThread }
            pending = invoke
            worker.execute {
                try {
                    val file = MarkdownExportStore(File(activity.cacheDir, "headstate-export")).prepare(args.markdown, kind = kind)
                    activity.runOnUiThread {
                        if (pending !== invoke) return@runOnUiThread
                        try {
                            val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.headstate.export", file)
                            val send = Intent(Intent.ACTION_SEND).apply {
                                type = kind.mime
                                putExtra(Intent.EXTRA_STREAM, uri)
                                clipData = ClipData.newUri(activity.contentResolver, kind.filename, uri)
                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            }
                            startActivityForResult(invoke, Intent.createChooser(send, "Share report").apply { addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION) }, "shared")
                        } catch (_: Exception) { finish(invoke, "failed") }
                    }
                } catch (error: Exception) {
                    activity.runOnUiThread { finish(invoke, if (error is ExportCapacityException) "capacity" else "failed") }
                }
            }
        }
    }
    @ActivityCallback
    fun shared(invoke: Invoke, result: ActivityResult) {
        // RESULT_CANCELED does not prove no receiver has read the URI.
        // Keep files for their lease; chooser return is never a saved claim.
        activity.runOnUiThread { finish(invoke, "presented") }
    }
    override fun onDestroy(activity: AppCompatActivity) {
        pending?.let { finish(it, "failed") }
        worker.shutdown()
    }
    private fun finish(invoke: Invoke, outcome: String) {
        if (pending !== invoke) return
        pending = null
        reply(invoke, outcome)
    }
    private fun reply(invoke: Invoke, outcome: String) { invoke.resolve(JSObject().put("outcome", outcome)) }
}
