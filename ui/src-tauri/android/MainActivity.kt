package com.grafium.app

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.Settings
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private lateinit var folderPickerLauncher: ActivityResultLauncher<Uri?>
  private var webViewRef: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { view, windowInsets ->
      val insets = windowInsets.getInsets(WindowInsetsCompat.Type.statusBars())
      view.setPadding(0, insets.top, 0, 0)
      windowInsets
    }

    folderPickerLauncher = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri: Uri? ->
      if (uri != null) {
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        try {
          contentResolver.takePersistableUriPermission(uri, flags)
        } catch (_: Exception) {}

        val path = treeUriToPath(uri)
        val result = path ?: uri.toString()
        webViewRef?.post {
          webViewRef?.evaluateJavascript(
            "window.__FOLDER_PICKER_RESOLVE && window.__FOLDER_PICKER_RESOLVE('${result.replace("'", "\\'")}')",
            null
          )
        }
      } else {
        webViewRef?.post {
          webViewRef?.evaluateJavascript(
            "window.__FOLDER_PICKER_RESOLVE && window.__FOLDER_PICKER_RESOLVE(null)",
            null
          )
        }
      }
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    webViewRef = webView
    webView.addJavascriptInterface(FolderPickerBridge(), "FolderPickerBridge")
  }

  inner class FolderPickerBridge {
    @JavascriptInterface
    fun pickFolder() {
      runOnUiThread {
        folderPickerLauncher.launch(null)
      }
    }

    @JavascriptInterface
    fun hasPersistentStorageAccess(): Boolean {
      return Build.VERSION.SDK_INT < Build.VERSION_CODES.R || Environment.isExternalStorageManager()
    }

    @JavascriptInterface
    fun requestPersistentStorageAccess() {
      if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
      runOnUiThread {
        val appSettings = Intent(
          Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
          Uri.parse("package:$packageName")
        )
        try {
          startActivity(appSettings)
        } catch (_: Exception) {
          startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))
        }
      }
    }
  }

  private fun treeUriToPath(uri: Uri): String? {
    if (DocumentsContract.isTreeUri(uri)) {
      val docId = DocumentsContract.getTreeDocumentId(uri)
      val split = docId.split(":")
      if (split.size > 1 && "primary".equals(split[0], ignoreCase = true)) {
        return "${Environment.getExternalStorageDirectory()}/${split[1]}"
      }
      if (split.size == 1 && "primary".equals(split[0], ignoreCase = true)) {
        return Environment.getExternalStorageDirectory().absolutePath
      }
    }
    return null
  }
}
