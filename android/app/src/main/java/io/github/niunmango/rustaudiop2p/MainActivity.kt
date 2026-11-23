package io.github.niunmango.rustaudiop2p

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
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

@Composable
fun RustAudioAndroidScreen(
    filesDirPath: String,
    cacheDirPath: String,
    isSpanish: Boolean,
    initialStatus: String,
    onDetectPolicy: (Boolean, Boolean) -> NetworkBatteryPolicy,
) {
    var selectedTab by remember { mutableIntStateOf(0) }
    var searchQuery by remember { mutableStateOf("") }
    var peerTicket by remember { mutableStateOf("") }
    var wifiOnlySync by remember { mutableStateOf(true) }
    var chargingOnlySeed by remember { mutableStateOf(true) }
    var statusText by remember { mutableStateOf("Node: $initialStatus") }

    val tabs = if (isSpanish) {
        listOf("Biblioteca", "Playlists & Viaje", "Red P2P", "Batería/Red")
    } else {
        listOf("Library", "Playlists & Travel", "P2P Net", "Battery/Net")
    }

    Scaffold(
        topBar = {
            Column {
                Text(
                    text = "rust-audio-p2p Mobile",
                    style = MaterialTheme.typography.titleLarge,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(14.dp),
                )
                TabRow(selectedTabIndex = selectedTab) {
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
                .padding(12.dp)
        ) {
            Text(statusText, color = MaterialTheme.colorScheme.secondary)
            Spacer(Modifier.height(10.dp))

            when (selectedTab) {
                0 -> {
                    OutlinedTextField(
                        value = searchQuery,
                        onValueChange = {
                            searchQuery = it
                            val res = runCatching {
                                RustAudioBridge.searchTracks(filesDirPath, cacheDirPath, it)
                            }.getOrNull()
                            statusText = res?.toString() ?: statusText
                        },
                        label = { Text(if (isSpanish) "Buscar (FTS5)" else "Search (FTS5)") },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(8.dp))
                    LazyColumn(modifier = Modifier.weight(1f)) {
                        item {
                            Text(
                                if (isSpanish) "Sincronizá tu biblioteca Linux por Wi-Fi o pines de viaje."
                                else "Sync your Linux library over Wi-Fi or travel pins.",
                                color = Color.LightGray,
                            )
                        }
                    }
                }
                1 -> {
                    Text(
                        if (isSpanish) "Playlists y Pines Offline para Modo Viaje"
                        else "Playlists & Offline Travel Pins",
                        style = MaterialTheme.typography.titleMedium,
                    )
                }
                2 -> {
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
                    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(if (isSpanish) "Descargar/Sincronizar en lote solo con Wi-Fi" else "Batch download/sync on Wi-Fi only")
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
                        Text(if (isSpanish) "Compartir/Seedear solo cargando batería" else "Share/Seed only while charging")
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
