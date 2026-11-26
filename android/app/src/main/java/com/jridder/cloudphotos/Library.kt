package com.jridder.cloudphotos

import java.io.File
import org.json.JSONArray
import org.json.JSONObject

data class Resource(val url: String, val checksum: String?, val width: Int, val height: Int)

/** One photo or video, as the helper describes it (photos_helper.asset_dict). */
data class Asset(
    val id: String,
    val master: String,
    val filename: String,
    val isVideo: Boolean,
    /** Unix seconds. */
    val taken: Double,
    val width: Int,
    val height: Int,
    val favorite: Boolean,
    val live: Boolean,
    /** Seconds, for videos. */
    val duration: Double?,
    val resources: Map<String, Resource>,
    val json: JSONObject,
) {
    /** The same asset with fresh download URLs ({key: url} from the helper's `urls`). */
    fun withUrls(urls: JSONObject): Asset {
        val json = JSONObject(json.toString())
        val res = json.optJSONObject("resources") ?: return this
        for (key in urls.keys()) {
            res.optJSONObject(key)?.put("url", urls.getString(key))
        }
        return parse(json) ?: this
    }

    companion object {
        fun parse(o: JSONObject): Asset? {
            val id = o.optString("id").ifEmpty { return null }
            val resources = buildMap {
                val res = o.optJSONObject("resources") ?: JSONObject()
                for (key in res.keys()) {
                    val r = res.optJSONObject(key) ?: continue
                    val url = r.optString("url").ifEmpty { continue }
                    put(key, Resource(url, r.optString("checksum").ifEmpty { null }, r.optInt("width"), r.optInt("height")))
                }
            }
            val details = o.optJSONObject("details")
            return Asset(
                id = id,
                master = o.optString("master"),
                filename = o.optString("filename"),
                isVideo = o.optString("kind") == "video",
                taken = o.optDouble("taken", 0.0),
                width = o.optInt("width"),
                height = o.optInt("height"),
                favorite = o.optBoolean("favorite"),
                live = o.optBoolean("live"),
                // Apple keeps it in QuickTime's 1/600 s units.
                duration = details?.optDouble("duration")?.takeIf { !it.isNaN() && it > 0 }?.div(600),
                resources = resources,
                json = o,
            )
        }
    }
}

/** An album as the helper describes it: smart (Favorites, Videos…) or the user's own. */
data class Album(val id: String, val name: String, val smart: Boolean, val assetIds: List<String>) {
    fun toJson(): JSONObject = JSONObject()
        .put("id", id)
        .put("name", name)
        .put("kind", if (smart) "smart" else "user")
        .put("assets", JSONArray(assetIds))

    companion object {
        fun parse(o: JSONObject): Album? {
            val id = o.optString("id").ifEmpty { return null }
            val ids = o.optJSONArray("assets") ?: JSONArray()
            return Album(id, o.optString("name", id), o.optString("kind") == "smart", (0 until ids.length()).map(ids::getString))
        }

        fun parseAll(array: JSONArray?): List<Album> =
            if (array == null) emptyList() else (0 until array.length()).mapNotNull { parse(array.getJSONObject(it)) }
    }
}

/** Newest first, as Photos shows them. */
fun List<Asset>.newestFirst(): List<Asset> = sortedByDescending { it.taken }

/**
 * The library as last seen, so the grid shows at once and later launches
 * only ask iCloud what changed (the sync token).
 */
class LibraryStore(dir: File) {
    private val file = File(dir, "library.json")

    data class Saved(val syncToken: String?, val appleId: String?, val assets: List<Asset>, val albums: List<Album>)

    fun load(): Saved? = try {
        val o = JSONObject(file.readText())
        val items = o.getJSONArray("assets")
        Saved(
            o.optString("sync_token").ifEmpty { null },
            o.optString("apple_id").ifEmpty { null },
            (0 until items.length()).mapNotNull { Asset.parse(items.getJSONObject(it)) },
            Album.parseAll(o.optJSONArray("albums")),
        )
    } catch (e: Exception) {
        null
    }

    fun save(syncToken: String?, appleId: String?, assets: List<Asset>, albums: List<Album>) {
        val o = JSONObject()
        o.put("sync_token", syncToken ?: "")
        o.put("apple_id", appleId ?: "")
        o.put("assets", JSONArray().apply { assets.forEach { put(it.json) } })
        o.put("albums", JSONArray().apply { albums.forEach { put(it.toJson()) } })
        val tmp = File(file.path + ".tmp")
        tmp.writeText(o.toString())
        tmp.renameTo(file)
    }

    fun clear() {
        file.delete()
    }
}
