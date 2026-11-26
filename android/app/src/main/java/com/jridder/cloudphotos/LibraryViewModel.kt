package com.jridder.cloudphotos

import android.app.Application
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

sealed interface Screen {
    data object Starting : Screen
    data class SignIn(val reason: String? = null, val error: String? = null, val busy: Boolean = false) : Screen
    data class Code(val error: String? = null, val busy: Boolean = false) : Screen
    data object Library : Screen
}

class LibraryViewModel(app: Application) : AndroidViewModel(app) {
    private val helper = (app as CloudPhotosApp).helper
    private val store = LibraryStore(app.filesDir)
    private val account = File(app.filesDir, "account.json")
    private val sessionDir = File(app.filesDir, "session")

    val media = Media(app.cacheDir) { renewLinks(it) }

    private val _screen = MutableStateFlow<Screen>(Screen.Starting)
    val screen: StateFlow<Screen> = _screen

    private val _assets = MutableStateFlow<List<Asset>>(emptyList())
    val assets: StateFlow<List<Asset>> = _assets

    /** What's happening in the background ("Loading your library…"), or null. */
    private val _status = MutableStateFlow<String?>(null)
    val status: StateFlow<String?> = _status

    private val _appleId = MutableStateFlow<String?>(null)
    val appleId: StateFlow<String?> = _appleId

    private val _messages = Channel<String>(Channel.BUFFERED)
    val messages = _messages.receiveAsFlow()

    private var syncToken: String? = null
    private var connected = false
    private var syncing: Job? = null

    init {
        viewModelScope.launch { start() }
    }

    private suspend fun start() {
        val saved = withContext(Dispatchers.IO) { store.load() }
        if (saved != null && account.exists()) {
            syncToken = saved.syncToken
            _appleId.value = saved.appleId
            _assets.value = saved.assets.newestFirst()
            _screen.value = Screen.Library
        }
        if (!account.exists()) {
            withContext(Dispatchers.IO) { store.clear() }
            _screen.value = Screen.SignIn()
            return
        }
        if (connect()) sync()
    }

    /** Resumes the saved session; false (and the right screen) if that fails. */
    private suspend fun connect(): Boolean {
        _status.value = "Connecting to iCloud…"
        try {
            val result = helper.call("connect", params("account" to account.path, "session_dir" to sessionDir.path))
            _appleId.value = result.optString("apple_id").ifEmpty { null }
            connected = true
            _screen.value = Screen.Library
            return true
        } catch (e: HelperException) {
            when (e.code) {
                "not_configured" -> _screen.value = Screen.SignIn()
                "needs_signin" -> _screen.value = Screen.SignIn(reason = "iCloud needs you to sign in again.")
                else -> {
                    // Offline, most likely: keep showing what's saved.
                    _screen.value = Screen.Library
                    _messages.send("Couldn't reach iCloud. ${e.message}")
                }
            }
            return false
        } finally {
            _status.value = null
        }
    }

    fun refresh() {
        viewModelScope.launch {
            if (connected || connect()) sync()
        }
    }

    private fun sync() {
        if (syncing?.isActive == true) return
        syncing = viewModelScope.launch {
            try {
                val token = syncToken
                if (token == null) {
                    scan()
                } else {
                    val changes = helper.call("changes", params("since" to token))
                    if (changes.optBoolean("full_scan")) scan() else apply(changes)
                }
            } catch (e: HelperException) {
                failed(e)
            } finally {
                _status.value = null
            }
        }
    }

    private suspend fun scan() {
        val found = ArrayList<Asset>()
        val showAsTheyCome = _assets.value.isEmpty()
        var lastShown = 0L
        _status.value = "Loading your library…"
        val result = helper.call("scan") { event ->
            if (event.optString("event") != "assets") return@call
            val items = event.optJSONArray("items") ?: return@call
            synchronized(found) {
                for (i in 0 until items.length()) Asset.parse(items.getJSONObject(i))?.let(found::add)
                _status.value = "Loading your library… ${"%,d".format(found.size)} items"
                val now = SystemClock.elapsedRealtime()
                if (showAsTheyCome && now - lastShown > 1000) {
                    lastShown = now
                    _assets.value = found.newestFirst()
                }
            }
        }
        val deleted = ids(result.optJSONArray("recently_deleted"))
        val assets = synchronized(found) { found.filter { it.id !in deleted }.newestFirst() }
        syncToken = result.optString("sync_token").ifEmpty { null }
        _assets.value = assets
        save()
    }

    private suspend fun apply(changes: JSONObject) {
        val byId = LinkedHashMap<String, Asset>()
        _assets.value.associateByTo(byId) { it.id }
        val updated = changes.optJSONArray("assets") ?: JSONArray()
        for (i in 0 until updated.length()) {
            Asset.parse(updated.getJSONObject(i))?.let { byId[it.id] = it }
        }
        (ids(changes.optJSONArray("trashed")) + ids(changes.optJSONArray("removed"))).forEach(byId::remove)
        syncToken = changes.optString("sync_token").ifEmpty { syncToken }
        _assets.value = byId.values.toList().newestFirst()
        save()
    }

    private suspend fun save() {
        val token = syncToken
        val appleId = _appleId.value
        val assets = _assets.value
        withContext(Dispatchers.IO) { store.save(token, appleId, assets) }
    }

    /** Fresh download links for [asset]; also kept in the library. */
    private suspend fun renewLinks(asset: Asset): Asset? = try {
        val result = helper.call(
            "urls",
            JSONObject().put("ids", JSONArray().put(asset.id)).put("masters", JSONObject().put(asset.id, asset.master)),
        )
        result.optJSONObject(asset.id)?.let { urls ->
            val fresh = asset.withUrls(urls)
            withContext(Dispatchers.Main) {
                _assets.value = _assets.value.map { if (it.id == asset.id) fresh else it }
            }
            fresh
        }
    } catch (e: HelperException) {
        null
    }

    private suspend fun failed(e: HelperException) {
        when (e.code) {
            "needs_signin" -> {
                connected = false
                _screen.value = Screen.SignIn(reason = "iCloud needs you to sign in again.")
            }
            else -> _messages.send(e.message ?: "Something went wrong.")
        }
    }

    // ── signing in ──────────────────────────────────────────────────────────

    fun signIn(username: String, password: String) {
        val reason = (_screen.value as? Screen.SignIn)?.reason
        _screen.value = Screen.SignIn(reason = reason, busy = true)
        viewModelScope.launch {
            try {
                val result = helper.call(
                    "signin",
                    params("username" to username.trim(), "password" to password, "account" to account.path, "session_dir" to sessionDir.path),
                )
                if (result.optBoolean("needs_code")) {
                    _screen.value = Screen.Code()
                } else {
                    signedIn(result)
                }
            } catch (e: HelperException) {
                _screen.value = Screen.SignIn(reason = reason, error = e.message)
            }
        }
    }

    fun verify(code: String) {
        _screen.value = Screen.Code(busy = true)
        viewModelScope.launch {
            try {
                signedIn(helper.call("verify", params("code" to code.trim())))
            } catch (e: HelperException) {
                _screen.value = Screen.Code(error = e.message)
            }
        }
    }

    fun cancelCode() {
        _screen.value = Screen.SignIn()
    }

    private suspend fun signedIn(result: JSONObject) {
        val appleId = result.optString("apple_id").ifEmpty { null }
        // Another Apple ID than the saved library's: start over.
        if (appleId != _appleId.value) {
            syncToken = null
            _assets.value = emptyList()
            withContext(Dispatchers.IO) { store.clear() }
        }
        _appleId.value = appleId
        connected = true
        _screen.value = Screen.Library
        sync()
    }

    fun signOut() {
        viewModelScope.launch {
            syncing?.cancel()
            connected = false
            syncToken = null
            _assets.value = emptyList()
            _appleId.value = null
            withContext(Dispatchers.IO) {
                account.delete()
                sessionDir.deleteRecursively()
                store.clear()
                media.clear()
            }
            _screen.value = Screen.SignIn()
        }
    }

    private fun params(vararg pairs: Pair<String, Any>) = JSONObject().apply { pairs.forEach { (k, v) -> put(k, v) } }

    private fun ids(array: JSONArray?): Set<String> = buildSet {
        if (array == null) return@buildSet
        for (i in 0 until array.length()) {
            when (val item = array.get(i)) {
                is JSONObject -> item.optString("id").ifEmpty { null }?.let(::add)
                is String -> add(item)
            }
        }
    }
}
