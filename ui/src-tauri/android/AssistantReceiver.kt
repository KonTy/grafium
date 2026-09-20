package com.grafium.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import org.json.JSONObject
import java.io.File

class AssistantReceiver : BroadcastReceiver() {
  companion object {
    private const val TAG = "GrafiumAssistant"
    private const val ACTION_EXECUTE_COMMAND = "com.silentpulse.action.EXECUTE_COMMAND"
    private const val ACTION_TTS_REPLY = "com.silentpulse.action.TTS_REPLY"
    private const val ACTION_REQUEST_SCHEMA = "com.silentpulse.action.REQUEST_SCHEMA"
    private const val ACTION_REPORT_SCHEMA = "com.silentpulse.action.REPORT_SCHEMA"
    private const val EXTRA_TRANSCRIPT = "EXTRA_TRANSCRIPT"
    private const val EXTRA_SESSION_ID = "EXTRA_SESSION_ID"
    private const val EXTRA_SPOKEN_TEXT = "EXTRA_SPOKEN_TEXT"
    private const val EXTRA_REQUIRE_FOLLOWUP = "EXTRA_REQUIRE_FOLLOWUP"
    private const val EXTRA_SCHEMA_JSON = "EXTRA_SCHEMA_JSON"
    private const val SILENTPULSE_PACKAGE = "com.silentpulse.messenger"
    private const val METADATA_DIR = ".grafium"

    init {
      try {
        System.loadLibrary("grafium_lib")
      } catch (error: Throwable) {
        Log.e(TAG, "Failed to load libgrafium_lib.so", error)
      }
    }

    @JvmStatic
    external fun nativeHandleCommand(transcript: String, graphPath: String): String
  }

  override fun onReceive(context: Context, intent: Intent) {
    when (intent.action) {
      ACTION_EXECUTE_COMMAND -> {
        val transcript = intent.getStringExtra(EXTRA_TRANSCRIPT) ?: return
        val sessionId = intent.getStringExtra(EXTRA_SESSION_ID) ?: ""
        dispatch(context, transcript, sessionId)
      }
      ACTION_REQUEST_SCHEMA -> sendSchema(context)
    }
  }

  private fun dispatch(context: Context, transcript: String, sessionId: String) {
    val graphDir = getGraphDir(context)
    if (graphDir == null) {
      reply(
        context,
        "I couldn't find a Grafium graph on this device. Open Grafium once to configure one.",
        sessionId,
        false,
      )
      return
    }

    val (speech, followup) = try {
      val response = JSONObject(nativeHandleCommand(transcript, graphDir.absolutePath))
      response.optString("speech", "Done.") to response.optBoolean("followup", false)
    } catch (error: Throwable) {
      Log.e(TAG, "nativeHandleCommand failed", error)
      "Sorry, I couldn't complete that." to false
    }
    reply(context, speech, sessionId, followup)
  }

  private fun reply(context: Context, text: String, sessionId: String, requireFollowUp: Boolean) {
    val intent = Intent(ACTION_TTS_REPLY).apply {
      setPackage(SILENTPULSE_PACKAGE)
      putExtra(EXTRA_SPOKEN_TEXT, text)
      putExtra(EXTRA_SESSION_ID, sessionId)
      putExtra(EXTRA_REQUIRE_FOLLOWUP, requireFollowUp)
    }
    context.sendBroadcast(intent)
  }

  private fun sendSchema(context: Context) {
    val schema = """
      {
        "app": "Grafium",
        "commands": [
          {"trigger": "add todo <text>", "description": "Add a TODO to today's journal (accepts priority + date suffixes)"},
          {"trigger": "add journal <text>", "description": "Add a note to today's journal"},
          {"trigger": "list my top <N> todos by priority", "description": "Speak the highest-priority open todos"},
          {"trigger": "list todos due today", "description": "Speak todos whose deadline is today"},
          {"trigger": "todos this week", "description": "Speak todos scheduled or due in the next 7 days"},
          {"trigger": "list my todos", "description": "Speak all open todos"},
          {"trigger": "find todo <query>", "description": "Search open todos by keyword"},
          {"trigger": "mark <query> done|doing|cancelled", "description": "Change a todo's state"},
          {"trigger": "read journal", "description": "Read today's journal entries"}
        ]
      }
    """.trimIndent()

    val intent = Intent(ACTION_REPORT_SCHEMA).apply {
      setPackage(SILENTPULSE_PACKAGE)
      putExtra(EXTRA_SCHEMA_JSON, schema)
    }
    context.sendBroadcast(intent)
  }

  private fun getGraphDir(context: Context): File? {
    val prefs = context.getSharedPreferences("grafium_prefs", Context.MODE_PRIVATE)
    val stored = prefs.getString("graph_path", null)
    if (stored != null) {
      val file = File(stored)
      if (file.exists() && File(file, "$METADATA_DIR/index.db").exists()) return file
    }

    val internalGraph = File(context.filesDir, "graph")
    if (File(internalGraph, "$METADATA_DIR/index.db").exists()) return internalGraph

    val externalGraph = context.getExternalFilesDir(null)?.let { File(it, "graph") }
    if (externalGraph != null && File(externalGraph, "$METADATA_DIR/index.db").exists()) {
      return externalGraph
    }

    try {
      val documents = File(
        android.os.Environment.getExternalStoragePublicDirectory(
          android.os.Environment.DIRECTORY_DOCUMENTS,
        ),
        "grafium",
      )
      documents.listFiles()?.filter { it.isDirectory }?.forEach { directory ->
        if (File(directory, "$METADATA_DIR/index.db").exists()) {
          prefs.edit().putString("graph_path", directory.absolutePath).apply()
          return directory
        }
      }
    } catch (_: Exception) {
      // The main UI offers the persistent storage grant when access is absent.
    }
    return null
  }
}
