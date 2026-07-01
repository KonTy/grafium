package com.grafium.companion.ink

import android.content.Context
import android.graphics.*
import android.util.AttributeSet
import android.view.MotionEvent
import android.view.View
import androidx.input.motionprediction.MotionEventPredictor
import java.util.UUID

/**
 * High-performance stylus canvas for handwriting capture.
 *
 * Features:
 * - Pressure-sensitive stroke rendering
 * - Palm rejection (via FLAG_CANCELED / ACTION_CANCEL)
 * - Finger vs stylus discrimination (stylus writes, finger scrolls)
 * - Motion prediction for reduced perceived latency
 * - Undo/redo support
 */
class InkCanvasView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
    defStyleAttr: Int = 0
) : View(context, attrs, defStyleAttr) {

    // --- Stroke data structures ---

    data class StrokePoint(
        val x: Float,
        val y: Float,
        val pressure: Float,
        val tilt: Float,
        val timestampMs: Long
    )

    data class Stroke(
        val id: String = UUID.randomUUID().toString(),
        val points: MutableList<StrokePoint> = mutableListOf(),
        val tool: PenTool = PenTool.PEN,
        val color: Int = Color.parseColor("#1a1a1a"),
        val baseWidth: Float = 3f
    )

    enum class PenTool { PEN, HIGHLIGHTER, ERASER }

    // --- State ---

    /** All completed strokes */
    private val strokes = mutableListOf<Stroke>()

    /** Strokes removed by undo (for redo) */
    private val undoStack = mutableListOf<Stroke>()

    /** Currently active stroke being drawn */
    private var currentStroke: Stroke? = null

    /** Timestamp when current stroke started (for relative timing) */
    private var strokeStartTime: Long = 0L

    // --- Configuration ---

    var currentTool: PenTool = PenTool.PEN
    var currentColor: Int = Color.parseColor("#1a1a1a")
    var currentWidth: Float = 3f

    // --- Rendering ---

    private val strokePaint = Paint().apply {
        isAntiAlias = true
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }

    private val highlighterPaint = Paint().apply {
        isAntiAlias = true
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        alpha = 102 // 0.4 * 255
    }

    /** Offscreen bitmap for completed strokes (avoids re-drawing all strokes every frame) */
    private var cacheBitmap: Bitmap? = null
    private var cacheCanvas: Canvas? = null
    private var cacheInvalid = true

    // --- Motion prediction ---

    private var motionPredictor: MotionEventPredictor? = null

    // --- Listener ---

    var onStrokeCompleted: ((Stroke) -> Unit)? = null
    var onStrokesChanged: (() -> Unit)? = null

    init {
        // Request unbuffered dispatch for lowest latency stylus input
        motionPredictor = MotionEventPredictor.newInstance(this)
    }

    // --- Public API ---

    fun getAllStrokes(): List<Stroke> = strokes.toList()

    fun getStrokeCount(): Int = strokes.size

    fun clearAll() {
        strokes.clear()
        undoStack.clear()
        cacheInvalid = true
        invalidate()
        onStrokesChanged?.invoke()
    }

    fun undo(): Boolean {
        if (strokes.isEmpty()) return false
        val removed = strokes.removeAt(strokes.lastIndex)
        undoStack.add(removed)
        cacheInvalid = true
        invalidate()
        onStrokesChanged?.invoke()
        return true
    }

    fun redo(): Boolean {
        if (undoStack.isEmpty()) return false
        val restored = undoStack.removeAt(undoStack.lastIndex)
        strokes.add(restored)
        cacheInvalid = true
        invalidate()
        onStrokesChanged?.invoke()
        return true
    }

    // --- Touch handling ---

    override fun onTouchEvent(event: MotionEvent): Boolean {
        // Only process stylus input for drawing
        // Finger input is ignored (parent handles scroll/pan)
        if (!isStylusEvent(event)) {
            return false // Let parent handle finger touches
        }

        motionPredictor?.record(event)

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                // Request unbuffered dispatch for lowest latency
                requestUnbufferedDispatch(event)
                startStroke(event)
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                continueStroke(event)
                return true
            }
            MotionEvent.ACTION_UP -> {
                finishStroke(event)
                return true
            }
            MotionEvent.ACTION_CANCEL -> {
                cancelStroke()
                return true
            }
        }
        return super.onTouchEvent(event)
    }

    private fun isStylusEvent(event: MotionEvent): Boolean {
        return event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS ||
               event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER
    }

    private fun startStroke(event: MotionEvent) {
        val tool = if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
            PenTool.ERASER
        } else {
            currentTool
        }

        strokeStartTime = event.eventTime
        currentStroke = Stroke(
            tool = tool,
            color = if (tool == PenTool.ERASER) Color.WHITE else currentColor,
            baseWidth = if (tool == PenTool.HIGHLIGHTER) currentWidth * 4f else currentWidth
        )

        addPointFromEvent(event)
        invalidate()
    }

    private fun continueStroke(event: MotionEvent) {
        val stroke = currentStroke ?: return

        // Process historical events (batched by the system for efficiency)
        for (i in 0 until event.historySize) {
            stroke.points.add(
                StrokePoint(
                    x = event.getHistoricalX(i),
                    y = event.getHistoricalY(i),
                    pressure = event.getHistoricalPressure(i).coerceIn(0f, 1f),
                    tilt = event.getHistoricalAxisValue(MotionEvent.AXIS_TILT, i),
                    timestampMs = event.getHistoricalEventTime(i) - strokeStartTime
                )
            )
        }
        addPointFromEvent(event)
        invalidate()
    }

    private fun finishStroke(event: MotionEvent) {
        val stroke = currentStroke ?: return
        addPointFromEvent(event)
        currentStroke = null

        if (stroke.tool == PenTool.ERASER) {
            eraseAt(stroke)
        } else if (stroke.points.size >= 2) {
            strokes.add(stroke)
            undoStack.clear() // New stroke invalidates redo history
            cacheInvalid = true
            onStrokeCompleted?.invoke(stroke)
            onStrokesChanged?.invoke()
        }
        invalidate()
    }

    private fun cancelStroke() {
        // Palm rejection or navigation gesture — discard current stroke
        currentStroke = null
        invalidate()
    }

    private fun addPointFromEvent(event: MotionEvent) {
        currentStroke?.points?.add(
            StrokePoint(
                x = event.x,
                y = event.y,
                pressure = event.pressure.coerceIn(0f, 1f),
                tilt = event.getAxisValue(MotionEvent.AXIS_TILT),
                timestampMs = event.eventTime - strokeStartTime
            )
        )
    }

    /** Simple proximity-based eraser: remove strokes that pass near the eraser path */
    private fun eraseAt(eraserStroke: Stroke) {
        val eraserRadius = eraserStroke.baseWidth * 3f
        val toRemove = mutableListOf<Stroke>()

        for (stroke in strokes) {
            for (ep in eraserStroke.points) {
                if (stroke.points.any { sp ->
                    val dx = sp.x - ep.x
                    val dy = sp.y - ep.y
                    dx * dx + dy * dy < eraserRadius * eraserRadius
                }) {
                    toRemove.add(stroke)
                    break
                }
            }
        }

        if (toRemove.isNotEmpty()) {
            strokes.removeAll(toRemove)
            cacheInvalid = true
            onStrokesChanged?.invoke()
        }
    }

    // --- Rendering ---

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        cacheBitmap?.recycle()
        cacheBitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        cacheCanvas = Canvas(cacheBitmap!!)
        cacheInvalid = true
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)

        // Draw cached completed strokes
        if (cacheInvalid) {
            rebuildCache()
        }
        cacheBitmap?.let { canvas.drawBitmap(it, 0f, 0f, null) }

        // Draw current in-progress stroke directly (no caching for responsiveness)
        currentStroke?.let { drawStroke(canvas, it) }

        // Draw predicted points (motion prediction for reduced perceived latency)
        motionPredictor?.predict()?.let { predicted ->
            currentStroke?.let { stroke ->
                val predPaint = getPaintForStroke(stroke).apply { alpha = 128 }
                val lastPoint = stroke.points.lastOrNull() ?: return@let
                canvas.drawLine(
                    lastPoint.x, lastPoint.y,
                    predicted.x, predicted.y,
                    predPaint
                )
            }
        }
    }

    private fun rebuildCache() {
        val cv = cacheCanvas ?: return
        cv.drawColor(Color.TRANSPARENT, PorterDuff.Mode.CLEAR)
        for (stroke in strokes) {
            drawStroke(cv, stroke)
        }
        cacheInvalid = false
    }

    private fun drawStroke(canvas: Canvas, stroke: Stroke) {
        if (stroke.points.size < 2) {
            // Single dot
            if (stroke.points.size == 1) {
                val p = stroke.points[0]
                val paint = getPaintForStroke(stroke)
                paint.style = Paint.Style.FILL
                canvas.drawCircle(p.x, p.y, stroke.baseWidth * p.pressure, paint)
                paint.style = Paint.Style.STROKE
            }
            return
        }

        val paint = getPaintForStroke(stroke)
        val path = Path()
        val points = stroke.points

        path.moveTo(points[0].x, points[0].y)

        // Draw smooth curve through points using quadratic bezier through midpoints
        for (i in 1 until points.size - 1) {
            val midX = (points[i].x + points[i + 1].x) / 2f
            val midY = (points[i].y + points[i + 1].y) / 2f
            path.quadTo(points[i].x, points[i].y, midX, midY)
        }
        // Final segment
        val last = points.last()
        path.lineTo(last.x, last.y)

        // Variable width based on average pressure
        val avgPressure = points.map { it.pressure }.average().toFloat()
        paint.strokeWidth = stroke.baseWidth * avgPressure * 2f

        canvas.drawPath(path, paint)
    }

    private fun getPaintForStroke(stroke: Stroke): Paint {
        val paint = if (stroke.tool == PenTool.HIGHLIGHTER) {
            Paint(highlighterPaint)
        } else {
            Paint(strokePaint)
        }
        paint.color = stroke.color
        return paint
    }
}
