package com.grafium.companion.ink

import android.os.Bundle
import android.view.View
import android.widget.*
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import java.io.File

/**
 * Full-screen ink capture activity.
 *
 * UX Flow:
 * - Stylus writes directly (no mode toggle needed)
 * - Finger scrolls/pans (handled by parent scroll view for multi-page)
 * - Toolbar: pen/highlighter/eraser tool selection, color, undo/redo
 * - Auto-saves SVG on pause/back
 * - "Convert to text" button triggers HTR and shows results
 */
class InkActivity : AppCompatActivity() {

    private lateinit var inkCanvas: InkCanvasView
    private lateinit var htrEngine: HtrEngine
    private lateinit var trainingManager: InkTrainingManager

    private var currentPageTitle: String = "untitled"
    private var graphRoot: File? = null
    private var inkFile: File? = null
    private var hasUnsavedChanges = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Get page info from intent
        currentPageTitle = intent.getStringExtra("page_title") ?: "untitled"
        graphRoot = intent.getStringExtra("graph_root")?.let { File(it) }

        // Set up the UI
        setupLayout()
        setupCallbacks()

        // Initialize HTR engine in background
        htrEngine = HtrEngine(this)
        trainingManager = InkTrainingManager(this)

        lifecycleScope.launch {
            htrEngine.initialize(HtrEngine.ModelTier.LITE)
        }

        // Load existing ink if editing an existing page
        loadExistingInk()
    }

    private fun setupLayout() {
        val rootLayout = FrameLayout(this).apply {
            layoutParams = FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT
            )
        }

        // Ink canvas (full screen)
        inkCanvas = InkCanvasView(this).apply {
            layoutParams = FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT
            )
            setBackgroundColor(0xFFFAFAFA.toInt())
        }
        rootLayout.addView(inkCanvas)

        // Bottom toolbar
        val toolbar = createToolbar()
        rootLayout.addView(toolbar)

        setContentView(rootLayout)
    }

    private fun createToolbar(): LinearLayout {
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                gravity = android.view.Gravity.BOTTOM
            }
            setPadding(16, 8, 16, 8)
            setBackgroundColor(0xF0FFFFFF.toInt())
            elevation = 8f

            // Pen button
            addView(createToolButton("Pen") {
                inkCanvas.currentTool = InkCanvasView.PenTool.PEN
            })

            // Highlighter button
            addView(createToolButton("Highlight") {
                inkCanvas.currentTool = InkCanvasView.PenTool.HIGHLIGHTER
            })

            // Eraser button
            addView(createToolButton("Eraser") {
                inkCanvas.currentTool = InkCanvasView.PenTool.ERASER
            })

            // Spacer
            addView(View(context).apply {
                layoutParams = LinearLayout.LayoutParams(0, 1, 1f)
            })

            // Undo
            addView(createToolButton("Undo") { inkCanvas.undo() })

            // Redo
            addView(createToolButton("Redo") { inkCanvas.redo() })

            // Spacer
            addView(View(context).apply {
                layoutParams = LinearLayout.LayoutParams(0, 1, 1f)
            })

            // Convert to text
            addView(createToolButton("Convert") { convertToText() })
        }
    }

    private fun createToolButton(label: String, onClick: () -> Unit): Button {
        return Button(this).apply {
            text = label
            textSize = 12f
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
            ).apply {
                marginEnd = 8
            }
            setOnClickListener { onClick() }
        }
    }

    private fun setupCallbacks() {
        inkCanvas.onStrokeCompleted = { _ ->
            hasUnsavedChanges = true
        }

        inkCanvas.onStrokesChanged = {
            hasUnsavedChanges = true
        }
    }

    /**
     * Load existing SVG ink file if this page already has one.
     */
    private fun loadExistingInk() {
        val root = graphRoot ?: return
        val file = InkSvgWriter.generateInkFilePath(root, currentPageTitle)
        if (file.exists()) {
            inkFile = file
            // TODO: Parse SVG back into strokes and load onto canvas
            // For now, we always create fresh (existing ink is preserved on disk)
        }
    }

    /**
     * Save current strokes to SVG file.
     */
    private fun saveInk() {
        if (!hasUnsavedChanges) return
        val root = graphRoot ?: return
        val strokes = inkCanvas.getAllStrokes()
        if (strokes.isEmpty()) return

        val file = InkSvgWriter.generateInkFilePath(root, currentPageTitle)
        InkSvgWriter.writeToFile(
            file = file,
            strokes = strokes,
            canvasWidth = inkCanvas.width.toFloat(),
            canvasHeight = inkCanvas.height.toFloat()
        )
        inkFile = file
        hasUnsavedChanges = false
    }

    /**
     * Run HTR on current strokes and show conversion result.
     */
    private fun convertToText() {
        val strokes = inkCanvas.getAllStrokes()
        if (strokes.isEmpty()) {
            Toast.makeText(this, "No handwriting to convert", Toast.LENGTH_SHORT).show()
            return
        }

        lifecycleScope.launch {
            val result = htrEngine.recognize(
                strokes = strokes,
                canvasWidth = inkCanvas.width.toFloat(),
                canvasHeight = inkCanvas.height.toFloat()
            )

            if (result != null) {
                showConversionDialog(result)
            } else {
                Toast.makeText(
                    this@InkActivity,
                    "Recognition failed. Is a model installed?",
                    Toast.LENGTH_LONG
                ).show()
            }
        }
    }

    /**
     * Show the conversion result and allow user to correct it.
     * Corrections are saved as training data for model personalization.
     */
    private fun showConversionDialog(result: HtrEngine.RecognitionResult) {
        val editText = EditText(this).apply {
            setText(result.text)
            setPadding(32, 16, 32, 16)
            hint = "Edit recognized text..."
        }

        android.app.AlertDialog.Builder(this)
            .setTitle("Recognized Text (${(result.confidence * 100).toInt()}% confidence)")
            .setView(editText)
            .setPositiveButton("Confirm") { _, _ ->
                val finalText = editText.text.toString()
                onTextConfirmed(finalText, result)
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    /**
     * Handle confirmed/corrected text.
     * If the user changed the text, store as a correction for training.
     */
    private fun onTextConfirmed(finalText: String, originalResult: HtrEngine.RecognitionResult) {
        // If user corrected the text, save as training data
        if (finalText != originalResult.text) {
            // Store correction pair for future fine-tuning
            val strokes = inkCanvas.getAllStrokes()
            val strokeIds = strokes.map { it.id }
            // TODO: Save correction to database via JNI/intent to core:
            // db.save_ink_correction(ink_id, stroke_ids, originalResult.text, finalText)
        }

        // Save the confirmed markdown alongside the SVG
        saveConvertedMarkdown(finalText)

        Toast.makeText(this, "Text saved to note", Toast.LENGTH_SHORT).show()
    }

    /**
     * Write the converted text as markdown, referencing the ink SVG.
     */
    private fun saveConvertedMarkdown(text: String) {
        val root = graphRoot ?: return
        val inkRelPath = inkFile?.relativeTo(root)?.path ?: return

        val mdFile = File(root, "pages/${currentPageTitle}.md")
        mdFile.parentFile?.mkdirs()

        val content = buildString {
            appendLine("# $currentPageTitle")
            appendLine()
            appendLine("![$currentPageTitle ink]($inkRelPath)")
            appendLine()
            appendLine("---")
            appendLine("*Recognized text:*")
            appendLine()
            appendLine(text)
        }

        mdFile.writeText(content)
    }

    override fun onPause() {
        super.onPause()
        saveInk()
    }

    override fun onDestroy() {
        super.onDestroy()
        htrEngine.close()
    }
}
