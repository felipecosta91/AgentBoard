package local.agentboard

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient

private const val FILE_CHOOSER = 1
private const val RETRY_MS = 6000L

class MainActivity : Activity() {
    private lateinit var web: WebView
    private var chooser: ValueCallback<Array<Uri>>? = null
    private var failed = true
    private var loadedBase: String? = null
    private val handler = Handler(Looper.getMainLooper())

    private val retry = object : Runnable {
        override fun run() {
            val base = Prefs.baseUrl(this@MainActivity)
            if (failed || base != loadedBase) load()
            handler.postDelayed(this, RETRY_MS)
        }
    }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!Prefs.paired(this)) {
            startActivity(Intent(this, PairActivity::class.java))
            finish()
            return
        }
        if (Build.VERSION.SDK_INT >= 33) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 0)
        PushService.start(this)

        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            // ngrok's free plan shows a browser interstitial unless the request doesn't look like a browser.
            settings.userAgentString = "AgentBoard/0.1 (Android WebView)"
            setBackgroundColor(0xFF0B0B0C.toInt())
            webViewClient = object : WebViewClient() {
                override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                    if (request.isForMainFrame) {
                        failed = true
                        view.loadData(offlinePage("Não foi possível conectar ao Mac (${error.description}). Tentando novamente…"), "text/html", "utf-8")
                    }
                }

                override fun onPageFinished(view: WebView, url: String) {
                    if (url.startsWith("https://")) failed = false
                }
            }
            webChromeClient = object : WebChromeClient() {
                override fun onShowFileChooser(view: WebView, cb: ValueCallback<Array<Uri>>, params: FileChooserParams): Boolean {
                    chooser?.onReceiveValue(null)
                    chooser = cb
                    return try {
                        startActivityForResult(params.createIntent(), FILE_CHOOSER)
                        true
                    } catch (e: Exception) {
                        chooser = null
                        false
                    }
                }
            }
        }
        setContentView(web)
    }

    override fun onResume() {
        super.onResume()
        if (Prefs.paired(this)) handler.post(retry)
    }

    override fun onPause() {
        handler.removeCallbacks(retry)
        super.onPause()
    }

    private fun load() {
        val base = Prefs.baseUrl(this)
        loadedBase = base
        if (base == null) {
            failed = true
            web.loadData(offlinePage("Aguardando o Mac ficar online…"), "text/html", "utf-8")
            return
        }
        failed = true // cleared by onPageFinished once the real page loads
        web.loadUrl("$base/?token=${Uri.encode(Prefs.token(this))}", mapOf("ngrok-skip-browser-warning" to "1"))
    }

    private fun offlinePage(message: String) =
        "<body style='background:#0b0b0c;color:#a1a1aa;font-family:sans-serif;display:grid;place-items:center;height:100vh;margin:0;text-align:center'><p>$message</p></body>"

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == FILE_CHOOSER) {
            chooser?.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(resultCode, data))
            chooser = null
        } else {
            super.onActivityResult(requestCode, resultCode, data)
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (::web.isInitialized && web.canGoBack()) web.goBack() else super.onBackPressed()
    }
}
