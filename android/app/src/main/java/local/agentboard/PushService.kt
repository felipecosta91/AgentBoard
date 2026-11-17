package local.agentboard

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

private const val CHANNEL_ALERTS = "alerts"
private const val CHANNEL_SERVICE = "service"
private const val URL_TAG = "agentboard-url"

/** Foreground service that follows the ntfy topic: shows pushes and learns the signed public URL. */
class PushService : Service() {
    @Volatile private var running = false
    private var worker: Thread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL_SERVICE, "Conexão", NotificationManager.IMPORTANCE_MIN))
        nm.createNotificationChannel(NotificationChannel(CHANNEL_ALERTS, "Atividade dos agentes", NotificationManager.IMPORTANCE_DEFAULT))
        val ongoing = Notification.Builder(this, CHANNEL_SERVICE)
            .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
            .setContentTitle("AgentBoard conectado")
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(1, ongoing, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(1, ongoing)

        if (!running) {
            running = true
            worker = Thread { loop() }.also { it.isDaemon = true; it.start() }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        worker?.interrupt()
        super.onDestroy()
    }

    private fun loop() {
        var backoff = 2000L
        while (running) {
            try {
                if (!Prefs.paired(this)) return
                if (Prefs.lastId(this) == null) catchUp()
                stream()
                backoff = 2000L
            } catch (e: InterruptedException) {
                return
            } catch (e: Exception) {
                // network down or server restarting: retry
            }
            try { Thread.sleep(backoff) } catch (e: InterruptedException) { return }
            backoff = (backoff * 2).coerceAtMost(30_000L)
        }
    }

    private fun open(path: String): HttpURLConnection {
        val conn = URL("${Prefs.server(this)}/${Prefs.topic(this)}/json$path").openConnection() as HttpURLConnection
        conn.connectTimeout = 15_000
        conn.readTimeout = 90_000 // ntfy sends a keepalive every ~30s
        return conn
    }

    /** First connection: learn the latest URL from the cache without replaying old notifications. */
    private fun catchUp() {
        val conn = open("?poll=1&since=12h")
        conn.inputStream.bufferedReader().useLines { lines -> lines.forEach { handle(it, silent = true) } }
        if (Prefs.lastId(this) == null) Prefs.setLastId(this, "latest")
    }

    private fun stream() {
        val conn = open("?since=${Prefs.lastId(this) ?: "latest"}")
        conn.inputStream.bufferedReader().useLines { lines ->
            for (line in lines) {
                if (!running) return
                handle(line, silent = false)
            }
        }
    }

    private fun handle(line: String, silent: Boolean) {
        val msg = try { JSONObject(line) } catch (e: Exception) { return }
        if (msg.optString("event") != "message") return
        msg.optString("id").takeIf { it.isNotEmpty() }?.let { Prefs.setLastId(this, it) }

        val tags = msg.optJSONArray("tags")
        val isUrl = (0 until (tags?.length() ?: 0)).any { tags!!.getString(it) == URL_TAG }
        if (isUrl) {
            acceptUrl(msg.optString("message"))
        } else if (!silent) {
            notify(msg.optString("title", "AgentBoard"), msg.optString("message"))
        }
    }

    /** Only HMAC-signed announcements (key = shared token) can change the URL the WebView loads. */
    private fun acceptUrl(body: String) {
        try {
            val j = JSONObject(body)
            val url = j.getString("u")
            val ts = j.getLong("ts")
            val mac = Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(Prefs.token(this@PushService)!!.toByteArray(), "HmacSHA256")) }
            val expected = mac.doFinal("$url|$ts".toByteArray()).joinToString("") { "%02x".format(it) }
            if (java.security.MessageDigest.isEqual(expected.toByteArray(), j.getString("sig").toByteArray()) &&
                url.startsWith("https://") && ts >= Prefs.urlTs(this)
            ) Prefs.setBaseUrl(this, url.trimEnd('/'), ts)
        } catch (e: Exception) {
            // malformed or forged announcement: ignore
        }
    }

    private fun notify(title: String, text: String) {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = Notification.Builder(this, CHANNEL_ALERTS)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        getSystemService(NotificationManager::class.java).notify((System.nanoTime() % Int.MAX_VALUE).toInt(), n)
    }

    companion object {
        fun start(context: Context) {
            val i = Intent(context, PushService::class.java)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(i) else context.startService(i)
        }
    }
}
