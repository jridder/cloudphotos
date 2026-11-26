package com.jridder.cloudphotos.ui

import androidx.activity.compose.BackHandler
import androidx.annotation.OptIn
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.MotionPhotosOn
import androidx.compose.material.icons.filled.PlayCircle
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.ui.PlayerView
import coil3.compose.AsyncImage
import com.jridder.cloudphotos.Asset
import com.jridder.cloudphotos.Media
import java.io.File
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

private val dateFormat = DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT)

/** What's playing: an asset's video ("original") or Live Photo motion ("live"). */
private data class Playing(val id: String, val key: String)

@Composable
fun ViewerScreen(
    assets: List<Asset>,
    start: Int,
    media: Media,
    link: suspend (Asset, String) -> String?,
    onMessage: (String) -> Unit,
    onClose: () -> Unit,
    onPage: (Int) -> Unit,
) {
    val pager = rememberPagerState(initialPage = start) { assets.size }
    var chrome by remember { mutableStateOf(true) }
    var playing by remember { mutableStateOf<Playing?>(null) }
    LaunchedEffect(pager) {
        snapshotFlow { pager.currentPage }.collect {
            playing = null
            onPage(it)
        }
    }
    BackHandler(enabled = playing != null) { playing = null }

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        // While a video plays, swipes go to its controls; Back stops it.
        HorizontalPager(pager, key = { assets[it].id }, beyondViewportPageCount = 1, userScrollEnabled = playing == null) { page ->
            val asset = assets[page]
            val now = playing?.takeIf { it.id == asset.id }
            if (now != null) {
                VideoPlayer(asset, now.key, link, onDone = { playing = null }, onError = {
                    onMessage(it)
                    playing = null
                })
            } else {
                Still(asset, media, onTap = { chrome = !chrome }, onPlay = { playing = Playing(asset.id, "original") })
            }
        }
        if (chrome || playing != null) {
            val asset = assets.getOrNull(pager.currentPage) ?: return@Box
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.6f), Color.Transparent)))
                    .statusBarsPadding()
                    .padding(4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = { if (playing != null) playing = null else onClose() }) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back", tint = Color.White)
                }
                Column(Modifier.weight(1f)) {
                    Text(
                        dateFormat.format(Instant.ofEpochMilli((asset.taken * 1000).toLong()).atZone(ZoneId.systemDefault())),
                        color = Color.White,
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(asset.filename, color = Color.White.copy(alpha = 0.75f), style = MaterialTheme.typography.bodySmall)
                }
                if (asset.live && !asset.isVideo && asset.resources.containsKey("live")) {
                    IconButton(onClick = { playing = Playing(asset.id, "live") }, enabled = playing == null) {
                        Icon(Icons.Filled.MotionPhotosOn, contentDescription = "Play Live Photo", tint = Color.White)
                    }
                }
                if (asset.favorite) {
                    Icon(Icons.Filled.Favorite, contentDescription = "Favorite", tint = Color.White, modifier = Modifier.padding(12.dp))
                }
            }
        }
    }
}

@Composable
private fun Still(asset: Asset, media: Media, onTap: () -> Unit, onPlay: () -> Unit) {
    // The thumbnail at once, then the larger preview.
    val thumb by produceState<File?>(media.cached(asset, "thumb"), asset.id) {
        if (value == null) value = media.file(asset, "thumb")
    }
    val preview by produceState<File?>(media.cached(asset, "preview"), asset.id) {
        if (value == null) value = media.file(asset, "preview")
    }
    Box(
        Modifier.fillMaxSize().clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onTap),
        contentAlignment = Alignment.Center,
    ) {
        (preview ?: thumb)?.let {
            AsyncImage(model = it, contentDescription = asset.filename, contentScale = ContentScale.Fit, modifier = Modifier.fillMaxSize())
        }
        if (asset.isVideo) {
            IconButton(onClick = onPlay, modifier = Modifier.size(88.dp)) {
                Icon(
                    Icons.Filled.PlayCircle,
                    contentDescription = "Play",
                    tint = Color.White.copy(alpha = 0.9f),
                    modifier = Modifier.size(80.dp),
                )
            }
        }
    }
}

/**
 * Streams [asset]'s [key] resource from iCloud with a fresh link. A video
 * gets the usual controls; a Live Photo's motion plays once, then the
 * still comes back.
 */
@OptIn(UnstableApi::class)
@Composable
private fun VideoPlayer(
    asset: Asset,
    key: String,
    link: suspend (Asset, String) -> String?,
    onDone: () -> Unit,
    onError: (String) -> Unit,
) {
    val context = LocalContext.current
    val player = remember { ExoPlayer.Builder(context).build() }
    var loading by remember { mutableStateOf(true) }
    val done by rememberUpdatedState(onDone)
    val failed by rememberUpdatedState(onError)

    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onPlaybackStateChanged(state: Int) {
                if (state == Player.STATE_READY) loading = false
                if (state == Player.STATE_ENDED && key == "live") done()
            }

            override fun onPlayerError(error: PlaybackException) {
                failed("Couldn't play this video (${error.errorCodeName}).")
            }
        }
        player.addListener(listener)
        onDispose {
            player.removeListener(listener)
            player.release()
        }
    }
    LaunchedEffect(asset.id, key) {
        val url = link(asset, key)
        if (url == null) {
            failed("Couldn't get this video from iCloud.")
            return@LaunchedEffect
        }
        player.setMediaItem(MediaItem.fromUri(url))
        player.prepare()
        player.playWhenReady = true
    }

    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        AndroidView(
            factory = {
                PlayerView(it).apply {
                    this.player = player
                    useController = key == "original"
                    keepScreenOn = true
                    setShowNextButton(false)
                    setShowPreviousButton(false)
                }
            },
            modifier = Modifier.fillMaxSize(),
        )
        if (loading) CircularProgressIndicator(color = Color.White)
    }
}
