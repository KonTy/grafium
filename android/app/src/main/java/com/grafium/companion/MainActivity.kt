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
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Button
import android.widget.Toast
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import com.grafium.companion.ink.InkActivity
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

        // Check for invalid graphs
        val invalidCount = checkInvalidGraphs()
        if (invalidCount > 0) {
            val warning = TextView(this).apply {
                text = "⚠ Found $invalidCount folder(s) without proper graph structure. " +
                       "These folders are missing pages/, journals/, and/or .grafium/ directories. " +
                       "Open Grafium and validate these folders, or delete them if they're not graphs.\n"
                textSize = 12f
                setTextColor(0xFFFF6600.toInt())
            }
            layout.addView(warning)
        }

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

        // --- Ink Canvas Section ---
        val inkHeader = TextView(this).apply {
            text = "\n✏️ Handwriting"
            textSize = 20f
        }
        layout.addView(inkHeader)

        val inkDesc = TextView(this).apply {
            text = "Open an infinite ink canvas. Write with your stylus, then convert to text on demand.\n"
            textSize = 14f
        }
        layout.addView(inkDesc)

        val inkTitleInput = EditText(this).apply {
            hint = "Page title (e.g. meeting-notes)"
            setSingleLine(true)
        }
        layout.addView(inkTitleInput)

        val inkBtn = Button(this).apply {
            text = "Open Ink Canvas"
            setOnClickListener {
                val pageTitle = inkTitleInput.text.toString().trim().ifEmpty { "ink-${System.currentTimeMillis()}" }
                val graphRoot = getActiveGraphRoot()
                if (graphRoot == null) {
                    Toast.makeText(this@MainActivity, "No graph found. Grant storage access first.", Toast.LENGTH_LONG).show()
                    return@setOnClickListener
                }
                startActivity(Intent(this@MainActivity, InkActivity::class.java).apply {
                    putExtra("page_title", pageTitle)
                    putExtra("graph_root", graphRoot.absolutePath)
                })
            }
        }
        layout.addView(inkBtn)

        setContentView(layout)
    }

    private fun checkInvalidGraphs(): Int {
        val grafiumRoot = File(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS),
            "grafium"
        )
        if (!grafiumRoot.exists()) return 0

        val allDirs = grafiumRoot.listFiles()?.filter { it.isDirectory } ?: return 0
        val validDirs = allDirs.filter { isValidGraphDir(it) }
        return allDirs.size - validDirs.size
    }

    private fun detectActiveGraph(): String {
        val graph = getActiveGraphRoot()
        if (graph == null) return "(none found)"

        val grafiumRoot = graph.parentFile ?: return graph.name
        val candidates = grafiumRoot.listFiles()?.filter { isValidGraphDir(it) } ?: return graph.name
        if (candidates.size == 1) return graph.name
        return "${graph.name} (auto-detected from ${candidates.size} graphs)"
    }

    private fun getActiveGraphRoot(): File? {
        val grafiumRoot = File(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS),
            "grafium"
        )
        if (!grafiumRoot.exists()) return null

        val candidates = grafiumRoot.listFiles()
            ?.filter { isValidGraphDir(it) }
            ?: return null

        if (candidates.isEmpty()) return null
        if (candidates.size == 1) return candidates[0]

        return candidates.maxByOrNull { dir ->
            File(dir, "journals").listFiles()
                ?.filter { it.name.endsWith(".md") }
                ?.maxOfOrNull { it.lastModified() } ?: 0L
        }
    }

    private fun isValidGraphDir(dir: File): Boolean {
        if (!dir.isDirectory) return false
        val pagesDir = File(dir, "pages")
        val journalsDir = File(dir, "journals")
         val metadataDir = File(dir, ".grafium")
        
        return pagesDir.isDirectory && 
               journalsDir.isDirectory && 
             metadataDir.isDirectory
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
