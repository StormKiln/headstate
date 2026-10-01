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

class ExportCapacityException : Exception()
class MarkdownExportStore(private val root: File) {
    fun prepare(markdown: String, now: Long = System.currentTimeMillis()): File {
        val bytes = markdown.toByteArray(Charsets.UTF_8)
        require(bytes.size <= 8 * 1024 * 1024) { "Export exceeds 8 MiB" }
        check(root.isDirectory || root.mkdirs()) { "Export storage unavailable" }
        fun entries() = root.listFiles()?.filter { it.name.matches(Regex("[a-f0-9]{64}")) }
            ?: throw IllegalStateException("Export storage unavailable")
        entries().filter { now - it.lastModified() > 24 * 60 * 60 * 1000L }.forEach {
            check(it.deleteRecursively()) { "Export cleanup unavailable" }
        }
        val hash = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        val folder = File(root, hash)
        val file = File(folder, "transcript.md")
        if (file.isFile) { check(folder.setLastModified(now)); return file }
        val live = entries()
        if (live.size >= 8 || live.sumOf { File(it, "transcript.md").length() } + bytes.size > 32 * 1024 * 1024) throw ExportCapacityException()
        check(folder.mkdir())
        try {
            file.outputStream().use { it.write(bytes); it.fd.sync() }
            check(folder.setLastModified(now))
            return file
        } catch (error: Exception) { folder.deleteRecursively(); throw error }
    }
}

@InvokeArg
class ShareArgs { lateinit var markdown: String }
@TauriPlugin
class HeadstateExportPlugin(private val activity: Activity) : Plugin(activity) {
    private val worker = Executors.newSingleThreadExecutor()
    private var pending: Invoke? = null
    @Command
    fun share(invoke: Invoke) {
        val args = invoke.parseArgs(ShareArgs::class.java)
        activity.runOnUiThread {
            if (pending != null) { reply(invoke, "busy"); return@runOnUiThread }
            if (activity.isFinishing || activity.isDestroyed) { reply(invoke, "failed"); return@runOnUiThread }
            pending = invoke
            worker.execute {
                try {
                    val file = MarkdownExportStore(File(activity.cacheDir, "headstate-export")).prepare(args.markdown)
                    activity.runOnUiThread {
                        if (pending !== invoke) return@runOnUiThread
                        try {
                            val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.headstate.export", file)
                            val send = Intent(Intent.ACTION_SEND).apply {
                                type = "text/markdown"
                                putExtra(Intent.EXTRA_STREAM, uri)
                                clipData = ClipData.newUri(activity.contentResolver, "transcript.md", uri)
                                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                            }
                            startActivityForResult(invoke, Intent.createChooser(send, "Share markdown").apply { addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION) }, "shared")
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
