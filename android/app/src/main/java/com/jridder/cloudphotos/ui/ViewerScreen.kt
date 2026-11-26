package com.jridder.cloudphotos.ui

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
import androidx.compose.material.icons.filled.PlayCircle
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import com.jridder.cloudphotos.Asset
import com.jridder.cloudphotos.Media
import java.io.File
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

private val dateFormat = DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT)

@Composable
fun ViewerScreen(assets: List<Asset>, start: Int, media: Media, onClose: () -> Unit, onPage: (Int) -> Unit) {
    val pager = rememberPagerState(initialPage = start) { assets.size }
    var chrome by remember { mutableStateOf(true) }
    LaunchedEffect(pager) { snapshotFlow { pager.currentPage }.collect(onPage) }

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        HorizontalPager(pager, key = { assets[it].id }, beyondViewportPageCount = 1) { page ->
            Page(assets[page], media, onTap = { chrome = !chrome })
        }
        if (chrome) {
            val asset = assets.getOrNull(pager.currentPage) ?: return@Box
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = 0.6f), Color.Transparent)))
                    .statusBarsPadding()
                    .padding(4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onClose) {
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
                if (asset.favorite) {
                    Icon(Icons.Filled.Favorite, contentDescription = "Favorite", tint = Color.White, modifier = Modifier.padding(12.dp))
                }
            }
        }
    }
}

@Composable
private fun Page(asset: Asset, media: Media, onTap: () -> Unit) {
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
            Icon(
                Icons.Filled.PlayCircle,
                contentDescription = "Video",
                tint = Color.White.copy(alpha = 0.85f),
                modifier = Modifier.size(72.dp),
            )
        }
    }
}
