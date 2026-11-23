package io.github.niunmango.rustaudiop2p

import android.annotation.SuppressLint
import android.os.Bundle
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import java.util.Locale

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val initResult = runCatching { RustAudioBridge.initSession(this) }.getOrNull()
        val isSpanish = Locale.getDefault().language.lowercase().startsWith("es")

        setContent {
            MaterialTheme(
                colorScheme = darkColorScheme(
                    background = Color(0xFF11111B),
                    surface = Color(0xFF181825),
                    primary = Color(0xFF89DCEB),
                    secondary = Color(0xFFA6E3A1),
                )
            ) {
                RustAudioAndroidScreen(
                    filesDirPath = filesDir.absolutePath,
                    cacheDirPath = cacheDir.absolutePath,
                    isSpanish = isSpanish,
                    initialStatus = initResult?.optString("node_name", "android-node") ?: "offline",
                    onDetectPolicy = { wifiOnly, chargingOnly ->
                        RustAudioBridge.detectPolicy(this, wifiOnly, chargingOnly)
                    }
                )
            }
        }
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
fun RustAudioAndroidScreen(
    filesDirPath: String,
    cacheDirPath: String,
    isSpanish: Boolean,
    initialStatus: String,
    onDetectPolicy: (Boolean, Boolean) -> NetworkBatteryPolicy,
) {
    var selectedTab by remember { mutableIntStateOf(0) }
    var nodeUrlInput by remember { mutableStateOf("http://192.168.1.200:4261") }
    var activeNodeUrl by remember { mutableStateOf("http://192.168.1.200:4261") }
    var searchQuery by remember { mutableStateOf("") }
    var peerTicket by remember { mutableStateOf("") }
    var wifiOnlySync by remember { mutableStateOf(true) }
    var chargingOnlySeed by remember { mutableStateOf(true) }
    var statusText by remember { mutableStateOf("Rust Core: $initialStatus") }

    val tabs = if (isSpanish) {
        listOf("Reproductor LAN", "FTS5 Local", "Red P2P", "Batería/Red")
    } else {
        listOf("LAN Player", "Local FTS5", "P2P Net", "Battery/Net")
    }

    Scaffold(
        topBar = {
            Column {
                Text(
                    text = "rust-audio-p2p Android",
                    style = MaterialTheme.typography.titleLarge,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
                )
                ScrollableTabRow(selectedTabIndex = selectedTab) {
                    tabs.forEachIndexed { idx, label ->
                        Tab(
                            selected = selectedTab == idx,
                            onClick = { selectedTab = idx },
                            text = { Text(label) },
                        )
                    }
                }
            }
        }
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(8.dp)
        ) {
            when (selectedTab) {
                0 -> {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        OutlinedTextField(
                            value = nodeUrlInput,
                            onValueChange = { nodeUrlInput = it },
                            label = { Text(if (isSpanish) "Nodo LAN (ej. 192.168.1.200:4261)" else "LAN Node URL") },
                            singleLine = true,
                            modifier = Modifier.weight(1f),
                        )
                        Button(
                            onClick = {
                                val trimmed = nodeUrlInput.trim()
                                activeNodeUrl = if (trimmed.startsWith("http")) trimmed else "http://$trimmed"
                            },
                            modifier = Modifier.padding(top = 8.dp),
                        ) {
                            Text(if (isSpanish) "Ir" else "Go")
                        }
                    }
                    Spacer(Modifier.height(6.dp))
                    AndroidView(
                        modifier = Modifier.fillMaxSize().weight(1f),
                        factory = { ctx ->
                            WebView(ctx).apply {
                                settings.javaScriptEnabled = true
                                settings.domStorageEnabled = true
                                settings.mediaPlaybackRequiresUserGesture = false
                                webViewClient = WebViewClient()
                                webChromeClient = WebChromeClient()
                                loadUrl(activeNodeUrl)
                            }
                        },
                        update = { webView ->
                            if (webView.url != activeNodeUrl) {
                                webView.loadUrl(activeNodeUrl)
                            }
                        },
                    )
                }
                1 -> {
                    Text(statusText, color = MaterialTheme.colorScheme.secondary)
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = searchQuery,
                        onValueChange = {
                            searchQuery = it
                            val res = runCatching {
                                RustAudioBridge.searchTracks(filesDirPath, cacheDirPath, it)
                            }.getOrNull()
                            statusText = res?.toString() ?: statusText
                        },
                        label = { Text(if (isSpanish) "Buscar (FTS5 Rust Core)" else "Search (FTS5 Rust Core)") },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    LazyColumn(modifier = Modifier.weight(1f)) {
                        item {
                            Text(
                                if (isSpanish) "Core SQLite + FTS5 y políticas offline activas en $filesDirPath"
                                else "SQLite + FTS5 core & offline cache active at $filesDirPath",
                                color = Color.LightGray,
                            )
                        }
                    }
                }
                2 -> {
                    Text(statusText, color = MaterialTheme.colorScheme.secondary)
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = peerTicket,
                        onValueChange = { peerTicket = it },
                        label = { Text(if (isSpanish) "Pegar ticket rap2p://..." else "Paste rap2p://... ticket") },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    Button(onClick = { statusText = "Ticket: ${peerTicket.take(42)}" }) {
                        Text(if (isSpanish) "Conectar Nodo" else "Connect Node")
                    }
                }
                3 -> {
                    Text(statusText, color = MaterialTheme.colorScheme.secondary)
                    Spacer(Modifier.height(10.dp))
                    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(if (isSpanish) "Sincronizar / descargar solo con Wi-Fi" else "Sync / download on Wi-Fi only")
                        Switch(
                            checked = wifiOnlySync,
                            onCheckedChange = {
                                wifiOnlySync = it
                                val pol = onDetectPolicy(wifiOnlySync, chargingOnlySeed)
                                RustAudioBridge.updatePolicy(filesDirPath, cacheDirPath, pol)
                            }
                        )
                    }
                    Spacer(Modifier.height(8.dp))
                    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(if (isSpanish) "Seedear P2P solo cargando batería" else "Seed P2P only while charging")
                        Switch(
                            checked = chargingOnlySeed,
                            onCheckedChange = {
                                chargingOnlySeed = it
                                val pol = onDetectPolicy(wifiOnlySync, chargingOnlySeed)
                                RustAudioBridge.updatePolicy(filesDirPath, cacheDirPath, pol)
                            }
                        )
                    }
                }
            }
        }
    }
}
