package com.grafium.companion

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Environment
import android.util.Log
import java.io.File
import java.time.LocalDate
import java.time.format.DateTimeFormatter

/**
 * Receives voice commands from SilentPulse via the cross-app assistant protocol.
 *
 * SilentPulse routes commands like "Computer, Grafium, journal meeting with Alice"
 * to this receiver. We parse the command, write to the active graph's journal,
 * and reply via TTS_REPLY so SilentPulse speaks confirmation.
 *
 * ## Protocol
 * - SilentPulse discovers us via ACTION_ASSISTANT_CAPABLE intent filter
 * - SilentPulse sends EXECUTE_COMMAND with EXTRA_TRANSCRIPT (raw command text)
 * - We process it and broadcast TTS_REPLY back with EXTRA_SPOKEN_TEXT
 */
class VoiceCommandReceiver : BroadcastReceiver() {

    companion object {
        private const val TAG = "GrafiumVoice"

        // SilentPulse protocol constants (must match CommandRouter.kt)
        const val ACTION_ASSISTANT_CAPABLE = "com.silentpulse.action.ASSISTANT_CAPABLE"
        const val ACTION_EXECUTE_COMMAND = "com.silentpulse.action.EXECUTE_COMMAND"
        const val ACTION_TTS_REPLY = "com.silentpulse.action.TTS_REPLY"
        const val ACTION_REQUEST_SCHEMA = "com.silentpulse.action.REQUEST_SCHEMA"
        const val ACTION_REPORT_SCHEMA = "com.silentpulse.action.REPORT_SCHEMA"

        const val EXTRA_TRANSCRIPT = "EXTRA_TRANSCRIPT"
        const val EXTRA_SESSION_ID = "EXTRA_SESSION_ID"
        const val EXTRA_SPOKEN_TEXT = "EXTRA_SPOKEN_TEXT"
        const val EXTRA_REQUIRE_FOLLOWUP = "EXTRA_REQUIRE_FOLLOWUP"
        const val EXTRA_SCHEMA_JSON = "EXTRA_SCHEMA_JSON"

        // Command patterns
        private val JOURNAL_PATTERN = Regex(
            "^(?:journal|note|log|jot|write down|add note|add entry|add to journal|add to today)\\s+(.+)",
            RegexOption.IGNORE_CASE
        )
        private val TODO_PATTERN = Regex(
            "^(?:todo|to do|task|add task|add todo|remind me to|reminder)\\s+(.+)",
            RegexOption.IGNORE_CASE
        )
        private val READ_JOURNAL_PATTERN = Regex(
            "^(?:read (?:my )?(?:journal|notes|entries)|what did i (?:note|write|log|journal)|today'?s (?:journal|notes|entries)|read today)",
            RegexOption.IGNORE_CASE
        )
        private val READ_TASKS_PATTERN = Regex(
            "^(?:read (?:my )?(?:tasks|todos|to-?do'?s?)|what are my (?:tasks|todos|to-?do'?s?)|pending (?:tasks|todos)|today'?s (?:tasks|todos))",
            RegexOption.IGNORE_CASE
        )
        private val PRIORITY_PATTERN = Regex(
            "\\s+priority\\s+(high|medium|low|urgent)\\s*$",
            RegexOption.IGNORE_CASE
        )
    }

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ACTION_ASSISTANT_CAPABLE -> {
                Log.d(TAG, "ASSISTANT_CAPABLE query — Grafium is alive")
            }
            ACTION_EXECUTE_COMMAND -> handleCommand(context, intent)
            ACTION_REQUEST_SCHEMA -> handleSchemaRequest(context)
            else -> Log.d(TAG, "Unknown action: ${intent.action}")
        }
    }

    private fun handleCommand(context: Context, intent: Intent) {
        val transcript = intent.getStringExtra(EXTRA_TRANSCRIPT) ?: return
        val sessionId = intent.getStringExtra(EXTRA_SESSION_ID) ?: "unknown"
        Log.d(TAG, "EXECUTE_COMMAND: \"$transcript\" session=$sessionId")

        val response = processCommand(context, transcript)
        Log.d(TAG, "Reply: \"$response\"")

        // Send TTS_REPLY back to SilentPulse
        context.sendBroadcast(Intent(ACTION_TTS_REPLY).apply {
            setPackage("com.silentpulse.messenger")
            putExtra(EXTRA_SPOKEN_TEXT, response)
            putExtra(EXTRA_SESSION_ID, sessionId)
            putExtra(EXTRA_REQUIRE_FOLLOWUP, false)
        })
    }

    private fun processCommand(context: Context, transcript: String): String {
        if (!hasStorageAccess()) {
            return "Grafium needs storage permission. Please open the Grafium companion app and grant All Files Access."
        }

        val trimmed = transcript.trim()

        // Read commands
        READ_JOURNAL_PATTERN.find(trimmed)?.let {
            return readTodayJournal(context)
        }
        READ_TASKS_PATTERN.find(trimmed)?.let {
            return readTodayTasks(context)
        }

        // TODO command (check before journal to avoid collision)
        TODO_PATTERN.find(trimmed)?.let { match ->
            return addTodo(context, match.groupValues[1].trim())
        }

        // Journal command
        JOURNAL_PATTERN.find(trimmed)?.let { match ->
            return addJournalEntry(context, match.groupValues[1].trim())
        }

        return "I didn't understand that. Try: journal, todo, read journal, or read tasks."
    }

    // ── Write commands ────────────────────────────────────────────────────

    private fun addJournalEntry(context: Context, text: String): String {
        val file = getTodayJournalFile(context) ?: return noGraphMessage()
        return try {
            file.parentFile?.mkdirs()
            file.appendText("- $text\n")
            val graphName = getActiveGraphName(context)
            "Added to today's journal in $graphName: $text"
        } catch (e: Exception) {
            Log.e(TAG, "Write failed", e)
            "Failed to write: ${e.message}"
        }
    }

    private fun addTodo(context: Context, rawText: String): String {
        val file = getTodayJournalFile(context) ?: return noGraphMessage()

        var text = rawText
        var priority: String? = null
        PRIORITY_PATTERN.find(text)?.let { match ->
            priority = match.groupValues[1].lowercase()
            text = text.substring(0, match.range.first).trim()
        }

        return try {
            file.parentFile?.mkdirs()
            file.appendText("- TODO $text\n")
            if (priority != null) {
                file.appendText("  priority:: $priority\n")
            }
            val graphName = getActiveGraphName(context)
            "Task added to $graphName: $text" + (priority?.let { ", priority $it" } ?: "")
        } catch (e: Exception) {
            Log.e(TAG, "Write failed", e)
            "Failed to write: ${e.message}"
        }
    }

    // ── Read commands ─────────────────────────────────────────────────────

    private fun readTodayJournal(context: Context): String {
        val file = getTodayJournalFile(context) ?: return noGraphMessage()
        if (!file.exists()) return "No journal entries for today yet."

        val entries = file.readLines()
            .filter { it.startsWith("- ") && !isTaskLine(it) }
            .map { it.removePrefix("- ").trim() }

        if (entries.isEmpty()) return "No journal entries for today."
        return "Today's journal: ${entries.joinToString(". ")}."
    }

    private fun readTodayTasks(context: Context): String {
        val file = getTodayJournalFile(context) ?: return noGraphMessage()
        if (!file.exists()) return "No tasks for today."

        val lines = file.readLines()
        val todos = lines.filter { it.trim().startsWith("- TODO ") }
            .map { it.trim().removePrefix("- TODO ").trim() }
        val doing = lines.filter { it.trim().startsWith("- DOING ") }
            .map { it.trim().removePrefix("- DOING ").trim() }

        val parts = mutableListOf<String>()
        if (todos.isNotEmpty()) parts.add("${todos.size} pending: ${todos.joinToString(", ")}")
        if (doing.isNotEmpty()) parts.add("${doing.size} in progress: ${doing.joinToString(", ")}")

        return if (parts.isEmpty()) "No tasks for today." else "Today's tasks. ${parts.joinToString(". ")}."
    }

    // ── Schema ────────────────────────────────────────────────────────────

    private fun handleSchemaRequest(context: Context) {
        val schema = """
            [
                {"command": "journal <text>", "description": "Add an entry to today's journal"},
                {"command": "todo <text>", "description": "Add a TODO task"},
                {"command": "todo <text> priority high", "description": "Add a prioritized task"},
                {"command": "read journal", "description": "Read today's journal entries"},
                {"command": "read tasks", "description": "Read today's pending tasks"}
            ]
        """.trimIndent()

        context.sendBroadcast(Intent(ACTION_REPORT_SCHEMA).apply {
            setPackage("com.silentpulse.messenger")
            putExtra(EXTRA_SCHEMA_JSON, schema)
        })
    }

    // ── Graph path resolution ─────────────────────────────────────────────

    private fun getTodayJournalFile(context: Context): File? {
        val graphDir = getActiveGraphDir(context) ?: return null
        val journalsDir = File(graphDir, "journals")

        val today = LocalDate.now()
        val dashName = today.format(DateTimeFormatter.ofPattern("yyyy-MM-dd")) + ".md"
        val underscoreName = today.format(DateTimeFormatter.ofPattern("yyyy_MM_dd")) + ".md"

        // If today's file already exists, use that format
        if (File(journalsDir, dashName).exists()) return File(journalsDir, dashName)
        if (File(journalsDir, underscoreName).exists()) return File(journalsDir, underscoreName)

        // Detect format from existing files
        val existingFiles = journalsDir.listFiles()?.map { it.name } ?: emptyList()
        val usesDashes = existingFiles.any { it.matches(Regex("\\d{4}-\\d{2}-\\d{2}\\.md")) }
        val filename = if (usesDashes) dashName else underscoreName
        return File(journalsDir, filename)
    }

    private fun getActiveGraphDir(context: Context): File? {
        // Check SharedPreferences for explicit path
        val prefs = context.getSharedPreferences("grafium_prefs", Context.MODE_PRIVATE)
        val saved = prefs.getString("active_graph_path", null)
        if (saved != null) {
            val dir = File(saved)
            if (dir.exists() && File(dir, "journals").isDirectory) return dir
        }

        // Auto-detect from Documents/grafium/
        val grafiumRoot = File(
            Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS),
            "grafium"
        )
        if (!grafiumRoot.exists()) return null

        val candidates = grafiumRoot.listFiles()
            ?.filter { it.isDirectory && File(it, "journals").isDirectory }
            ?: return null

        if (candidates.isEmpty()) return null
        if (candidates.size == 1) return candidates[0]

        // Multiple graphs — use the one with the most recent journal file
        val best = candidates.maxByOrNull { dir ->
            File(dir, "journals").listFiles()
                ?.filter { it.name.endsWith(".md") }
                ?.maxOfOrNull { it.lastModified() } ?: 0L
        }

        // Save for next time
        best?.let {
            prefs.edit().putString("active_graph_path", it.absolutePath).apply()
        }

        return best
    }

    private fun getActiveGraphName(context: Context): String {
        val dir = getActiveGraphDir(context) ?: return "unknown"
        return dir.name
    }

    private fun isTaskLine(line: String): Boolean {
        val t = line.trim()
        return t.startsWith("- TODO ") || t.startsWith("- DOING ") ||
               t.startsWith("- DONE ") || t.startsWith("- CANCELED ") ||
               t.startsWith("- LATER ") || t.startsWith("- NOW ")
    }

    private fun hasStorageAccess(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            true // Pre-R handled by manifest permission
        }
    }

    private fun noGraphMessage(): String {
        return "No Grafium graph found in Documents. Please create a graph first."
    }
}
