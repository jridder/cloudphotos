package com.jridder.cloudphotos.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil3.compose.AsyncImage
import com.jridder.cloudphotos.Asset
import com.jridder.cloudphotos.LibraryViewModel
import com.jridder.cloudphotos.Media
import java.io.File
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

@Composable
fun LibraryRoot(model: LibraryViewModel) {
    val assets by model.assets.collectAsStateWithLifecycle()
    var viewing by rememberSaveable { mutableStateOf<String?>(null) }
    val gridState = rememberLazyGridState()

    val open = viewing?.let { id -> assets.indexOfFirst { it.id == id }.takeIf { it >= 0 } }
    if (open != null) {
        BackHandler { viewing = null }
        ViewerScreen(assets, open, model.media, onClose = { viewing = null }, onPage = { viewing = assets[it].id })
    } else {
        LibraryScreen(model, assets, gridState, onOpen = { viewing = it.id })
    }
}

private sealed interface Cell {
    data class Header(val label: String) : Cell
    data class Photo(val asset: Asset) : Cell
}

private val monthFormat = DateTimeFormatter.ofPattern("LLLL yyyy")

/** The grid's cells: a header before each month's photos. */
private fun cells(assets: List<Asset>): List<Cell> {
    val zone = ZoneId.systemDefault()
    val out = ArrayList<Cell>(assets.size + 64)
    var month: String? = null
    for (asset in assets) {
        val label = monthFormat.format(Instant.ofEpochMilli((asset.taken * 1000).toLong()).atZone(zone))
        if (label != month) {
            month = label
            out.add(Cell.Header(label))
        }
        out.add(Cell.Photo(asset))
    }
    return out
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LibraryScreen(
    model: LibraryViewModel,
    assets: List<Asset>,
    gridState: androidx.compose.foundation.lazy.grid.LazyGridState,
    onOpen: (Asset) -> Unit,
) {
    val status by model.status.collectAsStateWithLifecycle()
    val appleId by model.appleId.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scroll = TopAppBarDefaults.enterAlwaysScrollBehavior()
    var menu by remember { mutableStateOf(false) }
    val cells = remember(assets) { cells(assets) }

    LaunchedEffect(model) { model.messages.collect { snackbar.showSnackbar(it) } }

    // iCloud sends the library oldest first, so newer photos keep arriving
    // above what's on screen; stay at the top (the newest) unless scrolled.
    var atTop by remember { mutableStateOf(true) }
    LaunchedEffect(gridState) {
        snapshotFlow { gridState.isScrollInProgress }.collect { scrolling ->
            if (!scrolling) atTop = gridState.firstVisibleItemIndex == 0 && gridState.firstVisibleItemScrollOffset == 0
        }
    }
    LaunchedEffect(cells) { if (atTop) gridState.scrollToItem(0) }

    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            Column {
                TopAppBar(
                    title = {
                        Column {
                            Text("Photos")
                            if (assets.isNotEmpty()) {
                                Text(
                                    "%,d items".format(assets.size),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                        }
                    },
                    actions = {
                        IconButton(onClick = model::refresh, enabled = status == null) {
                            Icon(Icons.Filled.Refresh, contentDescription = "Refresh")
                        }
                        Box {
                            IconButton(onClick = { menu = true }) { Icon(Icons.Filled.MoreVert, contentDescription = "More") }
                            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                                appleId?.let {
                                    DropdownMenuItem(text = { Text(it) }, onClick = {}, enabled = false)
                                }
                                DropdownMenuItem(text = { Text("Sign Out") }, onClick = {
                                    menu = false
                                    model.signOut()
                                })
                            }
                        }
                    },
                    scrollBehavior = scroll,
                )
                status?.let {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                    )
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        if (assets.isEmpty() && status == null) {
            Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
                Text("No photos yet", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            return@Scaffold
        }
        LazyVerticalGrid(
            columns = GridCells.Adaptive(96.dp),
            state = gridState,
            contentPadding = PaddingValues(
                top = padding.calculateTopPadding(),
                bottom = padding.calculateBottomPadding() + 8.dp,
            ),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            items(
                cells,
                key = { cell -> if (cell is Cell.Header) "month:${cell.label}" else (cell as Cell.Photo).asset.id },
                span = { cell -> if (cell is Cell.Header) GridItemSpan(maxLineSpan) else GridItemSpan(1) },
                contentType = { cell -> if (cell is Cell.Header) "header" else "photo" },
            ) { cell ->
                when (cell) {
                    is Cell.Header -> Text(
                        cell.label,
                        style = MaterialTheme.typography.titleSmall,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.padding(start = 12.dp, top = 16.dp, bottom = 6.dp),
                    )
                    is Cell.Photo -> Thumbnail(cell.asset, model.media, onClick = { onOpen(cell.asset) })
                }
            }
        }
    }
}

private val badge = TextStyle(
    color = Color.White,
    fontSize = 12.sp,
    fontWeight = FontWeight.SemiBold,
    shadow = Shadow(Color.Black.copy(alpha = 0.6f), blurRadius = 4f),
)

@Composable
private fun Thumbnail(asset: Asset, media: Media, onClick: () -> Unit) {
    val file by produceState<File?>(media.cached(asset, "thumb"), asset.id, asset.resources["thumb"]?.checksum) {
        if (value == null) value = media.file(asset, "thumb")
    }
    Box(
        Modifier
            .aspectRatio(1f)
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClick = onClick),
    ) {
        file?.let {
            AsyncImage(model = it, contentDescription = asset.filename, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        }
        if (asset.isVideo) {
            Text(
                asset.duration?.let(::formatDuration) ?: "Video",
                style = badge,
                modifier = Modifier.align(Alignment.BottomEnd).padding(5.dp),
            )
        }
        if (asset.favorite) {
            Icon(
                Icons.Filled.Favorite,
                contentDescription = "Favorite",
                tint = Color.White,
                modifier = Modifier.align(Alignment.BottomStart).padding(5.dp).size(14.dp),
            )
        }
    }
}

fun formatDuration(seconds: Double): String {
    val total = seconds.toLong()
    return if (total >= 3600) "%d:%02d:%02d".format(total / 3600, total / 60 % 60, total % 60)
    else "%d:%02d".format(total / 60, total % 60)
}
