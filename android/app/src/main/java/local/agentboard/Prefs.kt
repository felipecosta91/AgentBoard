package local.agentboard

import android.content.Context
import org.json.JSONObject

/** Pairing data (ntfy server/topic + shared token) and the last verified public URL. */
object Prefs {
    private fun sp(c: Context) = c.getSharedPreferences("agentboard", Context.MODE_PRIVATE)

    fun server(c: Context) = sp(c).getString("server", null)
    fun topic(c: Context) = sp(c).getString("topic", null)
    fun token(c: Context) = sp(c).getString("token", null)
    fun baseUrl(c: Context) = sp(c).getString("baseUrl", null)
    fun urlTs(c: Context) = sp(c).getLong("urlTs", 0)
    fun lastId(c: Context) = sp(c).getString("lastId", null)
    fun paired(c: Context) = server(c) != null && topic(c) != null && token(c) != null

    fun setLastId(c: Context, id: String) = sp(c).edit().putString("lastId", id).apply()
    fun setBaseUrl(c: Context, url: String, ts: Long) = sp(c).edit().putString("baseUrl", url).putLong("urlTs", ts).apply()

    /** Accepts the JSON shown by the desktop app: {"s": server, "t": topic, "k": token}. */
    fun pair(c: Context, raw: String): Boolean = try {
        val j = JSONObject(raw.trim())
        val s = j.getString("s").trimEnd('/')
        require(s.startsWith("https://") || s.startsWith("http://"))
        sp(c).edit().clear().putString("server", s).putString("topic", j.getString("t")).putString("token", j.getString("k")).apply()
        true
    } catch (e: Exception) {
        false
    }
}
