package com.jridder.cloudphotos

import android.app.Application

class CloudPhotosApp : Application() {
    /** One per process: it holds the signed-in iCloud session. */
    val helper by lazy { Helper(this) }
}
