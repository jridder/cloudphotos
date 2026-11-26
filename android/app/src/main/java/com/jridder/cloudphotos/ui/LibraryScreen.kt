package com.jridder.cloudphotos.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PhotoLibrary
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.outlined.PhotoAlbum
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import coil3.compose.AsyncImage
import com.jridder.cloudphotos.Album
import com.jridder.cloudphotos.Asset
import com.jridder.cloudphotos.LibraryViewModel
import com.jridder.cloudphotos.Media
import com.jridder.cloudphotos.newestFirst
import java.io.File
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

private const val ALL = "all"

/** photos_helper.SMART_ALBUMS */
private val SMART_ORDER = listOf("Favorites", "Screenshots", "Videos", "Live", "Panoramas", "Slo-mo", "Time-lapse", "Bursts")

/** The signed-in app: Library and Albums tabs, an album's photos, and the viewer. */
@Composable
fun LibraryRoot(model: LibraryViewModel) {
    val assets by model.assets.collectAsStateWithLifecycle()
    val albums by model.albums.collectAsStateWithLifecycle()
    var tab by rememberSaveable { mutableStateOf(ALL) }
    var openAlbum by rememberSaveable { mutableStateOf<String?>(null) }
    // The viewer: which list it pages through (ALL or an album id) and the item.
    var viewSource by rememberSaveable { mutableStateOf<String?>(null) }
    var viewing by rememberSaveable { mutableStateOf<String?>(null) }
    val libraryGrid = rememberLazyGridState()
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(model) { model.messages.collect { snackbar.showSnackbar(it) } }

    val byId = remember(assets) { assets.associateBy { it.id } }
    val album = openAlbum?.let { id -> albums.firstOrNull { it.id == id } }
    val albumAssets = remember(album, byId) { album?.let { a -> a.assetIds.mapNotNull(byId::get).newestFirst() } ?: emptyList() }

    val viewList = if (viewSource == ALL) assets else albumAssets
    val open = viewing?.let { id -> viewList.indexOfFirst { it.id == id }.takeIf { it >= 0 } }
    if (open != null) {
        BackHandler { viewing = null }
        ViewerScreen(
            viewList, open, model.media,
            link = model::link,
            onMessage = model::message,
            onClose = { viewing = null },
            onPage = { viewing = viewList[it].id },
        )
        return
    }

    Scaffold(
        bottomBar = {
            NavigationBar {
                NavigationBarItem(
                    selected = tab == ALL,
                    onClick = { tab = ALL },
                    icon = { Icon(Icons.Filled.PhotoLibrary, contentDescription = null) },
                    label = { Text("Library") },
                )
                NavigationBarItem(
                    selected = tab != ALL,
                    onClick = {
                        if (tab != ALL) openAlbum = null
                        tab = "albums"
                    },
                    icon = { Icon(Icons.Outlined.PhotoAlbum, contentDescription = null) },
                    label = { Text("Albums") },
                )
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
        // Each tab's own top bar handles the status bar.
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
    ) { outer ->
        val inner = Modifier.padding(bottom = outer.calculateBottomPadding()).consumeWindowInsets(outer)
        when {
            tab == ALL -> LibraryScreen(model, assets, libraryGrid, inner, onOpen = {
                viewSource = ALL
                viewing = it.id
            })
            album != null -> {
                BackHandler { openAlbum = null }
                key(album.id) {
                    AlbumScreen(album, albumAssets, model.media, inner, onBack = { openAlbum = null }, onOpen = {
                        viewSource = album.id
                        viewing = it.id
                    })
                }
            }
            else -> {
                BackHandler { tab = ALL }
                AlbumsScreen(albums, byId, model.media, inner, onOpen = { openAlbum = it.id })
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LibraryScreen(
    model: LibraryViewModel,
    assets: List<Asset>,
    gridState: LazyGridState,
    modifier: Modifier,
    onOpen: (Asset) -> Unit,
) {
    val status by model.status.collectAsStateWithLifecycle()
    val appleId by model.appleId.collectAsStateWithLifecycle()
    val scroll = TopAppBarDefaults.enterAlwaysScrollBehavior()
    var menu by remember { mutableStateOf(false) }

    // iCloud sends the library oldest first, so newer photos keep arriving
    // above what's on screen; stay at the top (the newest) unless scrolled.
    var atTop by remember { mutableStateOf(true) }
    LaunchedEffect(gridState) {
        snapshotFlow { gridState.isScrollInProgress }.collect { scrolling ->
            if (!scrolling) atTop = gridState.firstVisibleItemIndex == 0 && gridState.firstVisibleItemScrollOffset == 0
        }
    }
    LaunchedEffect(assets) { if (atTop) gridState.scrollToItem(0) }

    Scaffold(
        modifier = modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            Column {
                TopAppBar(
                    title = { Titled("Photos", if (assets.isEmpty()) null else count(assets.size)) },
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
    ) { padding ->
        if (assets.isEmpty() && status == null) {
            Empty("No photos yet", padding)
        } else {
            PhotoGrid(assets, model.media, gridState, padding, onOpen)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AlbumScreen(
    album: Album,
    assets: List<Asset>,
    media: Media,
    modifier: Modifier,
    onBack: () -> Unit,
    onOpen: (Asset) -> Unit,
) {
    val scroll = TopAppBarDefaults.enterAlwaysScrollBehavior()
    val grid = rememberLazyGridState()
    Scaffold(
        modifier = modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = {
            TopAppBar(
                title = { Titled(album.name, count(assets.size)) },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                },
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        if (assets.isEmpty()) Empty("This album is empty", padding) else PhotoGrid(assets, media, grid, padding, onOpen, months = false)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AlbumsScreen(
    albums: List<Album>,
    byId: Map<String, Asset>,
    media: Media,
    modifier: Modifier,
    onOpen: (Album) -> Unit,
) {
    val scroll = TopAppBarDefaults.enterAlwaysScrollBehavior()
    // Smart albums (Favorites, Videos…) first, in Photos' order, leaving out
    // empty ones; then the user's, by name.
    val sorted = remember(albums) {
        albums.filter { it.smart && it.assetIds.isNotEmpty() }.sortedBy { SMART_ORDER.indexOf(it.name).let { i -> if (i < 0) 99 else i } } +
            albums.filter { !it.smart }.sortedBy { it.name.lowercase() }
    }
    Scaffold(
        modifier = modifier.nestedScroll(scroll.nestedScrollConnection),
        topBar = { TopAppBar(title = { Text("Albums") }, scrollBehavior = scroll) },
    ) { padding ->
        if (sorted.isEmpty()) {
            Empty("No albums yet", padding)
            return@Scaffold
        }
        LazyVerticalGrid(
            columns = GridCells.Adaptive(160.dp),
            contentPadding = PaddingValues(
                start = 12.dp,
                end = 12.dp,
                top = padding.calculateTopPadding() + 4.dp,
                bottom = padding.calculateBottomPadding() + 12.dp,
            ),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            items(sorted, key = { it.id }) { album ->
                val members = remember(album, byId) { album.assetIds.mapNotNull(byId::get) }
                val cover = remember(members) { members.maxByOrNull { it.taken } }
                Column(Modifier.clickable { onOpen(album) }) {
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .aspectRatio(1f)
                            .background(MaterialTheme.colorScheme.surfaceVariant, MaterialTheme.shapes.medium),
                    ) {
                        cover?.let { Cover(it, media) }
                    }
                    Text(
                        album.name,
                        style = MaterialTheme.typography.titleSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 6.dp),
                    )
                    Text(
                        "%,d".format(members.size),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

@Composable
private fun Cover(asset: Asset, media: Media) {
    val file by produceState<File?>(media.cached(asset, "thumb"), asset.id) {
        if (value == null) value = media.file(asset, "thumb")
    }
    file?.let {
        AsyncImage(
            model = it,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize().clip(MaterialTheme.shapes.medium),
        )
    }
}

@Composable
private fun Titled(title: String, subtitle: String?) {
    Column {
        Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
        subtitle?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun Empty(text: String, padding: PaddingValues) {
    Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) {
        Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

private fun count(n: Int) = if (n == 1) "1 item" else "%,d items".format(n)

private sealed interface Cell {
    data class Header(val label: String) : Cell
    data class Photo(val asset: Asset) : Cell
}

private val monthFormat = DateTimeFormatter.ofPattern("LLLL yyyy")

/** The grid's cells: with [months], a header before each month's photos. */
private fun cells(assets: List<Asset>, months: Boolean): List<Cell> {
    if (!months) return assets.map(Cell::Photo)
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

/** Photos newest first, under month headers in the library (not in albums). */
@Composable
private fun PhotoGrid(
    assets: List<Asset>,
    media: Media,
    gridState: LazyGridState,
    padding: PaddingValues,
    onOpen: (Asset) -> Unit,
    months: Boolean = true,
) {
    val cells = remember(assets, months) { cells(assets, months) }
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
                is Cell.Photo -> Thumbnail(cell.asset, media, onClick = { onOpen(cell.asset) })
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
