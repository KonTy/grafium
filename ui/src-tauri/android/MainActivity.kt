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
import android.window.OnBackInvokedDispatcher
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private lateinit var folderPickerLauncher: ActivityResultLauncher<Uri?>
  private var webViewRef: WebView? = null
  private lateinit var readerLocationLauncher: ActivityResultLauncher<Intent>
  private lateinit var readerExportLauncher: ActivityResultLauncher<Intent>
  private lateinit var readerRestoreLauncher: ActivityResultLauncher<Intent>
  private var readerLocationRequest: String? = null
  private var readerExportRequest: String? = null
  private var readerRestoreRequest: String? = null
  private var readerBridge: PrivateReaderBridge? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    readerLocationLauncher = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
      val id = readerLocationRequest
      readerLocationRequest = null
      if (id != null) readerBridge?.selectedLocation(id,
        if (result.resultCode == RESULT_OK) result.data?.data else null)
    }
    readerExportLauncher = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
      val id = readerExportRequest
      readerExportRequest = null
      if (id != null) readerBridge?.selectedExport(id, if (result.resultCode == RESULT_OK) result.data?.data else null)
    }
    readerRestoreLauncher = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
      val id = readerRestoreRequest
      readerRestoreRequest = null
      if (id != null) readerBridge?.selectedRestore(id, if (result.resultCode == RESULT_OK) result.data?.data else null)
    }

    ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { view, windowInsets ->
      val insets = windowInsets.getInsets(WindowInsetsCompat.Type.statusBars())
      view.setPadding(0, insets.top, 0, 0)
      windowInsets
    }

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      onBackInvokedDispatcher.registerOnBackInvokedCallback(
        OnBackInvokedDispatcher.PRIORITY_DEFAULT
      ) {
        dispatchBackToGrafium()
      }
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
    webView.addJavascriptInterface(PrintBridge(this, webView), "GrafiumPrintBridge")
    readerBridge = PrivateReaderBridge(this, webView, { id ->
      check(readerLocationRequest == null) { "PICKER_ALREADY_OPEN" }
      readerLocationRequest = id
      readerLocationLauncher.launch(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
        putExtra(Intent.EXTRA_LOCAL_ONLY, true)
      })
    }, { id ->
      check(readerExportRequest == null) { "EXPORT_ALREADY_OPEN" }
      readerExportRequest = id
      readerExportLauncher.launch(Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
        addCategory(Intent.CATEGORY_OPENABLE)
        type = "application/json"
        putExtra(Intent.EXTRA_TITLE, "grafium-private-reader.json")
        putExtra(Intent.EXTRA_LOCAL_ONLY, true)
      })
    }, { id ->
      check(readerRestoreRequest == null) { "RESTORE_ALREADY_OPEN" }
      readerRestoreRequest = id
      readerRestoreLauncher.launch(Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
        addCategory(Intent.CATEGORY_OPENABLE)
        type = "application/json"
        putExtra(Intent.EXTRA_LOCAL_ONLY, true)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      })
    })
  }

  override fun onDestroy() {
    readerBridge?.destroy()
    readerBridge = null
    webViewRef = null
    super.onDestroy()
  }

  @Suppress("DEPRECATION")
  override fun onBackPressed() {
    dispatchBackToGrafium()
  }

  private fun dispatchBackToGrafium() {
    val webView = webViewRef ?: return
    webView.post {
      webView.evaluateJavascript(
        "window.dispatchEvent(new CustomEvent('grafium-android-back'))",
        null
      )
    }
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
