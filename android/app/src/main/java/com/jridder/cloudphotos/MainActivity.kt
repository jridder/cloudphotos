package com.jridder.cloudphotos

import android.content.pm.ApplicationInfo
import android.os.Bundle
import android.util.Log
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.jridder.cloudphotos.ui.CloudPhotosTheme
import com.jridder.cloudphotos.ui.CodeScreen
import com.jridder.cloudphotos.ui.LibraryRoot
import com.jridder.cloudphotos.ui.SignInScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0 && intent.getBooleanExtra("selftest", false)) {
            lifecycleScope.launch { Log.i("CloudPhotos", "selftest " + (application as CloudPhotosApp).helper.selftest()) }
        }
        setContent {
            CloudPhotosTheme {
                Surface(Modifier.fillMaxSize()) {
                    App(viewModel())
                }
            }
        }
    }
}

@Composable
private fun App(model: LibraryViewModel) {
    val screen by model.screen.collectAsStateWithLifecycle()
    when (val s = screen) {
        Screen.Starting -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        is Screen.SignIn -> SignInScreen(s, onSignIn = model::signIn)
        is Screen.Code -> CodeScreen(s, onVerify = model::verify, onBack = model::cancelCode)
        Screen.Library -> LibraryRoot(model)
    }
}
