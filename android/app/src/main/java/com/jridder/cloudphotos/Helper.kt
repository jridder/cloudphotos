package com.jridder.cloudphotos

import android.content.Context
import android.content.pm.ApplicationInfo
import com.chaquo.python.PyException
import com.chaquo.python.PyObject
import com.chaquo.python.Python
import com.chaquo.python.android.AndroidPlatform
import java.io.File
import java.util.TimeZone
import java.util.concurrent.Executors
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.withContext
import org.json.JSONObject

class HelperException(val code: String, message: String) : Exception(message)

/** Receives the helper's streamed events; called from Python. */
interface HelperListener {
    fun onEvent(json: String)
}

/**
 * The desktop app's photos_helper.py, run in this process through Chaquopy
 * (see android_bridge.py). Calls run one at a time on their own thread,
 * because pyicloud's session isn't safe to share.
 */
class Helper(private val context: Context) {
    private val dispatcher = Executors.newSingleThreadExecutor { Thread(it, "icloud-helper") }.asCoroutineDispatcher()
    private var bridge: PyObject? = null

    @Volatile
    private var onEvent: ((JSONObject) -> Unit)? = null

    /**
     * In debug builds, a fake library in the app's files (fake/library.json,
     * from tests/make_fake_library.py; see android/README.md) stands in for
     * iCloud, for tests and screenshots.
     */
    val fakeDir: File? = context.filesDir.resolve("fake").takeIf {
        context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0 && it.resolve("library.json").exists()
    }

    private fun bridge(): PyObject = bridge ?: run {
        if (!Python.isStarted()) Python.start(AndroidPlatform(context))
        val module = Python.getInstance().getModule("android_bridge")
        val listener = object : HelperListener {
            override fun onEvent(json: String) {
                onEvent?.invoke(JSONObject(json))
            }
        }
        module.callAttr("start", listener, context.filesDir.absolutePath, TimeZone.getDefault().id, fakeDir?.absolutePath)
        module.also { bridge = it }
    }

    /** Debug builds: loads pyicloud and its dependencies without signing in. */
    suspend fun selftest(): String = withContext(dispatcher) {
        try {
            "OK: " + bridge().callAttr("selftest").toString()
        } catch (e: PyException) {
            "FAILED: ${e.message}"
        }
    }

    /**
     * Calls a helper method (see photos_helper.py's docstring). [events]
     * gets each event the call streams, on the helper's thread, before the
     * call returns.
     */
    suspend fun call(
        method: String,
        params: JSONObject = JSONObject(),
        events: ((JSONObject) -> Unit)? = null,
    ): JSONObject = withContext(dispatcher) {
        onEvent = events
        try {
            val reply = JSONObject(bridge().callAttr("call", method, params.toString()).toString())
            reply.optJSONObject("error")?.let {
                throw HelperException(it.optString("code", "failed"), it.optString("message", "Something went wrong."))
            }
            reply.optJSONObject("result") ?: JSONObject()
        } catch (e: PyException) {
            throw HelperException("failed", e.message ?: "The iCloud helper failed.")
        } finally {
            onEvent = null
        }
    }
}
