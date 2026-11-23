package io.github.niunmango.rustaudiop2p

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

data class NetworkBatteryPolicy(
    val wifiOnlySync: Boolean = true,
    val chargingOnlySeed: Boolean = true,
    val isOnWifi: Boolean = false,
    val isCharging: Boolean = false,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("wifi_only_sync", wifiOnlySync)
        .put("charging_only_seed", chargingOnlySeed)
        .put("is_on_wifi", isOnWifi)
        .put("is_charging", isCharging)
}

object RustAudioBridge {
    init {
        System.loadLibrary("rust_audio_android_ffi")
    }

    external fun invokeJson(requestJson: String): String

    fun initSession(context: Context): JSONObject {
        val localeTag = Locale.getDefault().toLanguageTag()
        val req = JSONObject()
            .put("op", "open")
            .put("files_dir", context.filesDir.absolutePath)
            .put("cache_dir", context.cacheDir.absolutePath)
            .put("locale", localeTag)
        return JSONObject(invokeJson(req.toString()))
    }

    fun detectPolicy(context: Context, wifiOnlySync: Boolean, chargingOnlySeed: Boolean): NetworkBatteryPolicy {
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val caps = cm.getNetworkCapabilities(cm.activeNetwork)
        val onWifi = caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true ||
            caps?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true

        val batteryIntent = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val status = batteryIntent?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val charging = status == BatteryManager.BATTERY_STATUS_CHARGING ||
            status == BatteryManager.BATTERY_STATUS_FULL

        return NetworkBatteryPolicy(
            wifiOnlySync = wifiOnlySync,
            chargingOnlySeed = chargingOnlySeed,
            isOnWifi = onWifi,
            isCharging = charging,
        )
    }

    fun updatePolicy(filesDir: String, cacheDir: String, policy: NetworkBatteryPolicy): JSONObject {
        val req = JSONObject()
            .put("op", "set_policy")
            .put("files_dir", filesDir)
            .put("cache_dir", cacheDir)
            .put("locale", Locale.getDefault().toLanguageTag())
            .put("policy", policy.toJson())
        return JSONObject(invokeJson(req.toString()))
    }

    fun searchTracks(filesDir: String, cacheDir: String, query: String): JSONObject {
        val req = JSONObject()
            .put("op", "search")
            .put("files_dir", filesDir)
            .put("cache_dir", cacheDir)
            .put("locale", Locale.getDefault().toLanguageTag())
            .put("query", query)
        return JSONObject(invokeJson(req.toString()))
    }

    fun createPlaylistWithTracks(filesDir: String, cacheDir: String, name: String, ids: List<Long>): JSONObject {
        val req = JSONObject()
            .put("op", "create_playlist_with_tracks")
            .put("files_dir", filesDir)
            .put("cache_dir", cacheDir)
            .put("locale", Locale.getDefault().toLanguageTag())
            .put("name", name)
            .put("track_ids", JSONArray(ids))
        return JSONObject(invokeJson(req.toString()))
    }

    fun renamePlaylist(filesDir: String, cacheDir: String, playlistId: Long, newName: String): JSONObject {
        val req = JSONObject()
            .put("op", "rename_playlist")
            .put("files_dir", filesDir)
            .put("cache_dir", cacheDir)
            .put("locale", Locale.getDefault().toLanguageTag())
            .put("playlist_id", playlistId)
            .put("new_name", newName)
        return JSONObject(invokeJson(req.toString()))
    }

    fun deletePlaylist(filesDir: String, cacheDir: String, playlistId: Long): JSONObject {
        val req = JSONObject()
            .put("op", "delete_playlist")
            .put("files_dir", filesDir)
            .put("cache_dir", cacheDir)
            .put("locale", Locale.getDefault().toLanguageTag())
            .put("playlist_id", playlistId)
        return JSONObject(invokeJson(req.toString()))
    }
}

class PlaybackMediaService : MediaSessionService() {
    private var mediaSession: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        val player: Player = ExoPlayer.Builder(this).build()
        mediaSession = MediaSession.Builder(this, player).build()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = mediaSession

    override fun onDestroy() {
        mediaSession?.run {
            player.release()
            release()
            mediaSession = null
        }
        super.onDestroy()
    }
}
