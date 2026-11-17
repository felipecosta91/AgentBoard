package local.agentboard

import android.app.Activity
import android.content.Intent
import android.graphics.Color
import android.os.Bundle
import android.view.Gravity
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode

class PairActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pad = (24 * resources.displayMetrics.density).toInt()
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(pad, pad, pad, pad)
            setBackgroundColor(Color.parseColor("#0B0B0C"))
        }
        root.addView(TextView(this).apply {
            text = "AgentBoard"
            textSize = 28f
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
        })
        root.addView(TextView(this).apply {
            text = "No app do Mac, abra Celular e escaneie o QR (ou cole o código).\nFunciona em qualquer rede."
            setTextColor(Color.parseColor("#A1A1AA"))
            gravity = Gravity.CENTER
            setPadding(0, pad / 2, 0, pad)
        })
        root.addView(Button(this).apply {
            text = "Escanear QR"
            setOnClickListener { scan() }
        })
        val code = EditText(this).apply {
            hint = "ou cole o código de pareamento"
            setTextColor(Color.WHITE)
            setHintTextColor(Color.GRAY)
            maxLines = 3
        }
        root.addView(code)
        root.addView(Button(this).apply {
            text = "Conectar"
            setOnClickListener { finishPairing(code.text.toString()) }
        })
        setContentView(root)
    }

    private fun scan() {
        val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
        GmsBarcodeScanning.getClient(this, options).startScan()
            .addOnSuccessListener { finishPairing(it.rawValue ?: "") }
            .addOnFailureListener { Toast.makeText(this, "Não foi possível escanear: ${it.message}", Toast.LENGTH_LONG).show() }
    }

    private fun finishPairing(raw: String) {
        if (!Prefs.pair(this, raw)) {
            Toast.makeText(this, "Código inválido", Toast.LENGTH_LONG).show()
            return
        }
        startActivity(Intent(this, MainActivity::class.java))
        finish()
    }
}
