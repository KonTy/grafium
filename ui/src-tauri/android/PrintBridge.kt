package com.grafium.app

import android.content.Context
import android.print.PrintAttributes
import android.print.PrintManager
import android.webkit.JavascriptInterface
import android.webkit.WebView

/**
 * Printing on Android.
 *
 * The desktop builds drive WebKitGTK's own print operation from Rust, but
 * Android has no equivalent: `window.print()` is not implemented in Android's
 * WebView, so a page that relies on it silently does nothing. Printing here
 * has to go through the platform's print framework instead, which is only
 * reachable from Kotlin.
 *
 * Android's print dialog offers "Save as PDF" alongside the real printers, so
 * this one entry point covers both of the things the print dialog offers on
 * the desktop.
 */
class PrintBridge(private val activity: MainActivity, private val webView: WebView) {

  /**
   * Hand the current page to the system printer.
   *
   * The adapter prints the live document, which is what the frontend has
   * already prepared: the print document is mounted into the page and
   * `print.css` hides everything else, exactly as on the desktop.
   */
  @JavascriptInterface
  fun print(jobName: String) {
    activity.runOnUiThread {
      try {
        val manager = activity.getSystemService(Context.PRINT_SERVICE) as? PrintManager
        if (manager == null) {
          report(false, "Printing is unavailable on this device")
          return@runOnUiThread
        }
        val name = jobName.ifBlank { "Grafium" }
        manager.print(
          name,
          webView.createPrintDocumentAdapter(name),
          PrintAttributes.Builder().build(),
        )
        report(true, "")
      } catch (error: Exception) {
        report(false, error.message ?: "Could not start printing")
      }
    }
  }

  /** Whether the platform can print at all, so the UI can say so up front. */
  @JavascriptInterface
  fun isAvailable(): Boolean =
    activity.getSystemService(Context.PRINT_SERVICE) as? PrintManager != null

  private fun report(started: Boolean, message: String) {
    val escaped = message.replace("\\", "\\\\").replace("'", "\\'").replace("\n", " ")
    webView.post {
      webView.evaluateJavascript(
        "window.__GRAFIUM_PRINT_RESOLVE && window.__GRAFIUM_PRINT_RESOLVE($started, '$escaped')",
        null,
      )
    }
  }
}
