package com.grafium.companion

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.Settings
import android.app.Activity
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Button
import android.widget.Toast
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import java.io.File

/**
 * Minimal settings activity for Grafium companion.
 * Shows the active graph and lets you grant storage permission.
 */
class MainActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 48, 48, 48)
        }

        val title = TextView(this).apply {
            text = "Grafium Voice Companion"
            textSize = 24f
        }
        layout.addView(title)

        val desc = TextView(this).apply {
            text = "\nThis app receives voice commands from SilentPulse " +
                   "and writes journal entries and tasks to your Grafium graph.\n\n" +
                   "Say: \"Computer, Grafium, journal <your text>\"\n" +
                   "Or:  \"Computer, Grafium, todo <your task>\"\n"
            textSize = 16f
        }
        layout.addView(desc)

        val graphInfo = TextView(this).apply {
            text = "Active graph: ${detectActiveGraph()}\n"
            textSize = 14f
        }
        layout.addView(graphInfo)

        val storageStatus = TextView(this).apply {
            text = "Storage access: ${if (hasStorageAccess()) "✓ Granted" else "✗ Not granted"}\n"
            textSize = 14f
        }
        layout.addView(storageStatus)

        if (!hasStorageAccess()) {
            val warning = TextView(this).apply {
                text = "⚠ Storage access is REQUIRED for voice commands to work. " +
                       "Without it, Grafium cannot read or write journal files.\n"
                textSize = 14f
                setTextColor(0xFFCC0000.toInt())
            }
            layout.addView(warning)

            val btn = Button(this).apply {
                text = "Grant Storage Access"
                setOnClickListener { requestStorageAccess() }
            }
            layout.addView(btn)
        }

        setContentView(layout)
    }

    private fun detectActiveGraph(): String {
        val grafiumRoot = File(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS),
            "grafium"
        )
        if (!grafiumRoot.exists()) return "(none found)"

        val candidates = grafiumRoot.listFiles()
            ?.filter { it.isDirectory && File(it, "journals").isDirectory }
            ?: return "(none found)"

        if (candidates.isEmpty()) return "(none found)"
        if (candidates.size == 1) return candidates[0].name

        val best = candidates.maxByOrNull { dir ->
            File(dir, "journals").listFiles()
                ?.filter { it.name.endsWith(".md") }
                ?.maxOfOrNull { it.lastModified() } ?: 0L
        }
        return "${best?.name} (auto-detected from ${candidates.size} graphs)"
    }

    private fun hasStorageAccess(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            ContextCompat.checkSelfPermission(this, Manifest.permission.WRITE_EXTERNAL_STORAGE) ==
                PackageManager.PERMISSION_GRANTED
        }
    }

    private fun requestStorageAccess() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
                data = Uri.parse("package:$packageName")
            }
            startActivity(intent)
        } else {
            ActivityCompat.requestPermissions(
                this,
                arrayOf(Manifest.permission.WRITE_EXTERNAL_STORAGE),
                100
            )
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<String>, results: IntArray) {
        if (requestCode == 100 && results.isNotEmpty() && results[0] == PackageManager.PERMISSION_GRANTED) {
            Toast.makeText(this, "Storage access granted", Toast.LENGTH_SHORT).show()
            recreate()
        }
    }
}
