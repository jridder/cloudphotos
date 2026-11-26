package com.jridder.cloudphotos

import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.security.MessageDigest
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext

/** A download link iCloud no longer accepts (they expire after a while). */
class ExpiredLink : IOException("The download link expired.")

/**
 * Thumbnails and previews, downloaded once into the cache and kept by
 * asset and checksum (a new checksum means the photo was edited). An
 * expired link is renewed through [renew] and tried once more.
 */
class Media(cacheDir: File, private val renew: suspend (Asset) -> Asset?) {
    private val dir = File(cacheDir, "media").apply { mkdirs() }
    private val gate = Semaphore(6)
    private val lock = Mutex()
    private val inFlight = HashMap<String, CompletableDeferred<File?>>()

    fun cached(asset: Asset, kind: String): File? = resource(asset, kind)?.let { fileFor(asset, kind, it) }?.takeIf { it.exists() }

    suspend fun file(asset: Asset, kind: String): File? {
        val res = resource(asset, kind) ?: return null
        val target = fileFor(asset, kind, res)
        if (target.exists()) return target

        val (job, mine) = lock.withLock {
            inFlight[target.name]?.let { it to false } ?: (CompletableDeferred<File?>().also { inFlight[target.name] = it } to true)
        }
        if (!mine) {
            return try {
                job.await()
            } catch (e: CancellationException) {
                // The download was dropped (its thumbnail scrolled away); if
                // this one still wants the file, fetch it itself.
                currentCoroutineContext().ensureActive()
                file(asset, kind)
            }
        }
        try {
            val result = try {
                gate.withPermit { fetch(asset, kind, res.url, target) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                null
            }
            job.complete(result)
            return result
        } catch (e: CancellationException) {
            job.cancel()
            throw e
        } finally {
            withContext(NonCancellable) { lock.withLock { inFlight.remove(target.name) } }
        }
    }

    private suspend fun fetch(asset: Asset, kind: String, url: String, target: File): File? = try {
        download(url, target)
    } catch (e: ExpiredLink) {
        val fresh = renew(asset)?.let { resource(it, kind) } ?: return null
        download(fresh.url, target)
    }

    fun clear() {
        dir.deleteRecursively()
        dir.mkdirs()
    }

    private fun resource(asset: Asset, kind: String): Resource? = when (kind) {
        "preview" -> asset.resources["preview"] ?: asset.resources["thumb"]
        else -> asset.resources[kind]
    }

    private fun fileFor(asset: Asset, kind: String, res: Resource): File {
        val digest = MessageDigest.getInstance("SHA-1").digest("${asset.id}\u0000$kind\u0000${res.checksum}".toByteArray())
        return File(dir, digest.joinToString("") { "%02x".format(it) } + ".jpg")
    }

    private suspend fun download(url: String, target: File): File = withContext(Dispatchers.IO) {
        val tmp = File(target.path + ".part")
        if (url.startsWith("file:")) {
            // The fake library (debug builds).
            File(URI(url)).copyTo(tmp, overwrite = true)
        } else {
            val connection = URL(url).openConnection() as HttpURLConnection
            connection.connectTimeout = 15_000
            connection.readTimeout = 30_000
            try {
                when (connection.responseCode) {
                    in 200..299 -> connection.inputStream.use { input -> tmp.outputStream().use { input.copyTo(it) } }
                    401, 403, 404, 410 -> throw ExpiredLink()
                    else -> throw IOException("HTTP ${connection.responseCode}")
                }
            } finally {
                connection.disconnect()
            }
        }
        if (!tmp.renameTo(target)) throw IOException("Couldn't save ${target.name}")
        target
    }
}
