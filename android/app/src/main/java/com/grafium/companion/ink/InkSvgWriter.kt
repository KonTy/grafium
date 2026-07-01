package com.grafium.companion.ink

import java.io.File
import java.text.SimpleDateFormat
import java.util.*

/**
 * Serializes InkCanvasView strokes to SVG files on disk.
 * Format is compatible with the Rust core InkSvgParser for cross-platform roundtrip.
 *
 * The SVG includes:
 * - Standard SVG paths (viewable in any browser/editor)
 * - Grafium metadata namespace (gfm:*) for pressure, tilt, timestamps
 */
object InkSvgWriter {

    private const val GRAFIUM_NS = "https://grafium.app/ink/v1"

    /**
     * Serialize strokes to SVG string.
     */
    fun serialize(
        strokes: List<InkCanvasView.Stroke>,
        canvasWidth: Float,
        canvasHeight: Float,
        createdAt: Long = System.currentTimeMillis(),
        updatedAt: Long = System.currentTimeMillis()
    ): String {
        val sb = StringBuilder(strokes.size * 512)

        // SVG header
        sb.append("""<svg xmlns="http://www.w3.org/2000/svg" xmlns:gfm="$GRAFIUM_NS" viewBox="0 0 $canvasWidth $canvasHeight" width="${canvasWidth.toInt()}" height="${canvasHeight.toInt()}">""")
        sb.append('\n')

        // Metadata
        sb.append("""  <metadata><gfm:ink version="1" created="$createdAt" updated="$updatedAt" strokes="${strokes.size}"/></metadata>""")
        sb.append('\n')

        // Styles
        sb.append("  <defs>\n")
        sb.append("    <style>\n")
        sb.append("      path { fill: none; stroke-linecap: round; stroke-linejoin: round; }\n")
        sb.append("      .highlighter { opacity: 0.4; }\n")
        sb.append("    </style>\n")
        sb.append("  </defs>\n")

        // Strokes
        for (stroke in strokes) {
            serializeStroke(sb, stroke)
        }

        sb.append("</svg>\n")
        return sb.toString()
    }

    private fun serializeStroke(sb: StringBuilder, stroke: InkCanvasView.Stroke) {
        if (stroke.points.isEmpty()) return

        val pressureData = stroke.points.joinToString(",") { "%.2f".format(it.pressure) }
        val timestampData = stroke.points.joinToString(",") { it.timestampMs.toString() }
        val hasTilt = stroke.points.any { it.tilt > 0.01f }

        val toolStr = when (stroke.tool) {
            InkCanvasView.PenTool.PEN -> "pen"
            InkCanvasView.PenTool.HIGHLIGHTER -> "highlighter"
            InkCanvasView.PenTool.ERASER -> "eraser"
        }

        // Open group with metadata
        sb.append("""  <g id="${stroke.id}" gfm:tool="$toolStr" gfm:pressure="$pressureData" gfm:timestamps="$timestampData"""")

        if (hasTilt) {
            val tiltData = stroke.points.joinToString(",") { "%.2f".format(it.tilt) }
            sb.append(""" gfm:tilt="$tiltData"""")
        }
        sb.append(">\n")

        val colorHex = String.format("#%06x", stroke.color and 0xFFFFFF)
        val classAttr = if (stroke.tool == InkCanvasView.PenTool.HIGHLIGHTER) {
            """ class="highlighter""""
        } else ""

        if (stroke.points.size == 1) {
            // Single dot
            val p = stroke.points[0]
            val r = stroke.baseWidth * p.pressure * 0.5f
            sb.append("""    <circle cx="%.1f" cy="%.1f" r="%.1f" fill="$colorHex"$classAttr/>""".format(p.x, p.y, r))
            sb.append('\n')
        } else {
            // Polyline path (exact points for data fidelity)
            val pathD = buildPathData(stroke.points)
            val avgPressure = stroke.points.map { it.pressure }.average().toFloat()
            val renderedWidth = stroke.baseWidth * avgPressure

            sb.append("""    <path d="$pathD" stroke="$colorHex" stroke-width="%.1f"$classAttr/>""".format(renderedWidth))
            sb.append('\n')
        }

        sb.append("  </g>\n")
    }

    private fun buildPathData(points: List<InkCanvasView.StrokePoint>): String {
        val sb = StringBuilder(points.size * 16)
        sb.append("M %.1f %.1f".format(points[0].x, points[0].y))
        for (i in 1 until points.size) {
            sb.append(" L %.1f %.1f".format(points[i].x, points[i].y))
        }
        return sb.toString()
    }

    /**
     * Write strokes to an SVG file on disk.
     * Creates parent directories if needed.
     */
    fun writeToFile(
        file: File,
        strokes: List<InkCanvasView.Stroke>,
        canvasWidth: Float,
        canvasHeight: Float
    ) {
        file.parentFile?.mkdirs()
        val svg = serialize(strokes, canvasWidth, canvasHeight)
        file.writeText(svg)
    }

    /**
     * Generate a default ink file path based on current date and page title.
     */
    fun generateInkFilePath(graphRoot: File, pageTitle: String): File {
        val dateStr = SimpleDateFormat("yyyy-MM-dd", Locale.US).format(Date())
        val safeTitle = pageTitle.replace(Regex("[^a-zA-Z0-9_-]"), "_").take(50)
        return File(graphRoot, "assets/ink/${safeTitle}_$dateStr.svg")
    }
}
