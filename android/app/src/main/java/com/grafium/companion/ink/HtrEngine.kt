package com.grafium.companion.ink

import android.content.Context
import android.graphics.*
import ai.onnxruntime.*
import kotlinx.coroutines.*
import java.io.File
import java.nio.FloatBuffer

/**
 * On-device Handwriting Text Recognition engine using ONNX Runtime.
 *
 * Supports two model tiers:
 * - htr-lite.onnx (~5MB): Fast CRNN model, lower accuracy
 * - htr-full.onnx (~30-120MB): TrOCR-style model, higher accuracy
 *
 * The engine:
 * 1. Renders stroke regions to grayscale bitmaps
 * 2. Preprocesses (normalize, resize to model input size)
 * 3. Runs inference via ONNX Runtime (with NNAPI acceleration if available)
 * 4. Decodes output via CTC or autoregressive decoding
 *
 * Models are expected in: {app_files}/models/htr-lite.onnx or htr-full.onnx
 */
class HtrEngine(private val context: Context) {

    companion object {
        /** Model input height (standard for HTR models trained on IAM) */
        const val INPUT_HEIGHT = 64

        /** Max input width (wider images are split into segments) */
        const val MAX_INPUT_WIDTH = 800

        /** Character set for CTC decoding (ASCII + common symbols) */
        private val CHARSET = buildString {
            append(" ") // blank/CTC
            append("abcdefghijklmnopqrstuvwxyz")
            append("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
            append("0123456789")
            append(".,;:!?'-\"()[]{}/@#\$%&*+=<>~^_\\|`")
        }
    }

    private var ortEnv: OrtEnvironment? = null
    private var ortSession: OrtSession? = null
    private var modelLoaded = false
    private var modelTier: ModelTier = ModelTier.LITE

    enum class ModelTier { LITE, FULL }

    data class RecognitionResult(
        val text: String,
        val confidence: Float,
        val modelVersion: String
    )

    /**
     * Initialize the HTR engine. Call once at startup.
     * Returns false if no model file is found.
     */
    fun initialize(preferredTier: ModelTier = ModelTier.LITE): Boolean {
        try {
            ortEnv = OrtEnvironment.getEnvironment()

            val modelFile = getModelFile(preferredTier)
                ?: getModelFile(ModelTier.LITE)
                ?: return false

            val sessionOptions = OrtSession.SessionOptions().apply {
                // Try NNAPI for hardware acceleration (GPU/NPU on Android)
                try {
                    addNnapi()
                } catch (_: Exception) {
                    // NNAPI not available, fall back to CPU
                }
                setOptimizationLevel(OrtSession.SessionOptions.OptLevel.ALL_OPT)
                setIntraOpNumThreads(2)
            }

            ortSession = ortEnv?.createSession(modelFile.absolutePath, sessionOptions)
            modelLoaded = true
            modelTier = preferredTier
            return true
        } catch (e: Exception) {
            e.printStackTrace()
            return false
        }
    }

    /**
     * Recognize text from a list of strokes.
     * Renders strokes to a bitmap, preprocesses, and runs inference.
     */
    suspend fun recognize(
        strokes: List<InkCanvasView.Stroke>,
        canvasWidth: Float,
        canvasHeight: Float
    ): RecognitionResult? = withContext(Dispatchers.Default) {
        if (!modelLoaded || strokes.isEmpty()) return@withContext null

        try {
            // 1. Render strokes to grayscale bitmap
            val bitmap = renderStrokesToBitmap(strokes, canvasWidth, canvasHeight)

            // 2. Preprocess: crop to content, resize to model input size
            val processed = preprocessBitmap(bitmap)
            bitmap.recycle()

            // 3. Convert to float tensor
            val inputTensor = bitmapToTensor(processed)
            processed.recycle()

            // 4. Run inference
            val outputText = runInference(inputTensor)

            RecognitionResult(
                text = outputText.first,
                confidence = outputText.second,
                modelVersion = "${modelTier.name.lowercase()}-v1"
            )
        } catch (e: Exception) {
            e.printStackTrace()
            null
        }
    }

    /**
     * Render strokes to a grayscale bitmap (black ink on white background).
     */
    private fun renderStrokesToBitmap(
        strokes: List<InkCanvasView.Stroke>,
        canvasWidth: Float,
        canvasHeight: Float
    ): Bitmap {
        // Find bounding box of all strokes (with padding)
        var minX = Float.MAX_VALUE
        var minY = Float.MAX_VALUE
        var maxX = Float.MIN_VALUE
        var maxY = Float.MIN_VALUE

        for (stroke in strokes) {
            for (point in stroke.points) {
                minX = minOf(minX, point.x)
                minY = minOf(minY, point.y)
                maxX = maxOf(maxX, point.x)
                maxY = maxOf(maxY, point.y)
            }
        }

        val padding = 20f
        minX = maxOf(0f, minX - padding)
        minY = maxOf(0f, minY - padding)
        maxX = minOf(canvasWidth, maxX + padding)
        maxY = minOf(canvasHeight, maxY + padding)

        val width = (maxX - minX).toInt().coerceAtLeast(1)
        val height = (maxY - minY).toInt().coerceAtLeast(1)

        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        canvas.drawColor(Color.WHITE)

        val paint = Paint().apply {
            isAntiAlias = true
            style = Paint.Style.STROKE
            strokeCap = Paint.Cap.ROUND
            strokeJoin = Paint.Join.ROUND
            color = Color.BLACK
        }

        // Draw strokes offset to bounding box origin
        for (stroke in strokes) {
            if (stroke.points.size < 2) continue
            val path = Path()
            val points = stroke.points
            path.moveTo(points[0].x - minX, points[0].y - minY)

            for (i in 1 until points.size) {
                path.lineTo(points[i].x - minX, points[i].y - minY)
            }

            val avgPressure = points.map { it.pressure }.average().toFloat()
            paint.strokeWidth = stroke.baseWidth * avgPressure * 2f
            canvas.drawPath(path, paint)
        }

        return bitmap
    }

    /**
     * Preprocess: resize to model input height while maintaining aspect ratio.
     */
    private fun preprocessBitmap(bitmap: Bitmap): Bitmap {
        val scale = INPUT_HEIGHT.toFloat() / bitmap.height
        val targetWidth = (bitmap.width * scale).toInt().coerceIn(1, MAX_INPUT_WIDTH)

        return Bitmap.createScaledBitmap(bitmap, targetWidth, INPUT_HEIGHT, true)
    }

    /**
     * Convert grayscale bitmap to ONNX input tensor.
     * Shape: [1, 1, height, width] (batch, channel, H, W)
     * Values normalized to [0, 1] (0=black ink, 1=white background)
     */
    private fun bitmapToTensor(bitmap: Bitmap): OnnxTensor {
        val width = bitmap.width
        val height = bitmap.height
        val pixels = IntArray(width * height)
        bitmap.getPixels(pixels, 0, width, 0, 0, width, height)

        val floatData = FloatBuffer.allocate(width * height)
        for (pixel in pixels) {
            // Convert to grayscale and normalize to [0, 1]
            val gray = (Color.red(pixel) + Color.green(pixel) + Color.blue(pixel)) / (3f * 255f)
            floatData.put(gray)
        }
        floatData.rewind()

        val shape = longArrayOf(1, 1, height.toLong(), width.toLong())
        return OnnxTensor.createTensor(ortEnv, floatData, shape)
    }

    /**
     * Run ONNX inference and decode output.
     * Returns (recognized_text, confidence).
     */
    private fun runInference(inputTensor: OnnxTensor): Pair<String, Float> {
        val session = ortSession ?: return Pair("", 0f)

        val inputName = session.inputNames.first()
        val results = session.run(mapOf(inputName to inputTensor))

        // CTC output: [sequence_length, batch, num_classes]
        val outputTensor = results[0] as OnnxTensor
        val outputData = outputTensor.floatBuffer

        val shape = outputTensor.info.shape
        val seqLen = shape[0].toInt()
        val numClasses = shape[shape.size - 1].toInt()

        // CTC greedy decode
        val decoded = ctcGreedyDecode(outputData, seqLen, numClasses)

        results.close()
        inputTensor.close()

        return decoded
    }

    /**
     * CTC greedy decoding: pick highest probability class at each timestep,
     * collapse repeated characters, remove blanks.
     */
    private fun ctcGreedyDecode(
        output: FloatBuffer,
        seqLen: Int,
        numClasses: Int
    ): Pair<String, Float> {
        val sb = StringBuilder()
        var prevClass = -1
        var totalConfidence = 0f
        var charCount = 0

        for (t in 0 until seqLen) {
            var maxProb = Float.MIN_VALUE
            var maxClass = 0

            for (c in 0 until numClasses) {
                val prob = output.get(t * numClasses + c)
                if (prob > maxProb) {
                    maxProb = prob
                    maxClass = c
                }
            }

            // Class 0 = blank (CTC)
            if (maxClass != 0 && maxClass != prevClass) {
                if (maxClass - 1 < CHARSET.length) {
                    sb.append(CHARSET[maxClass - 1])
                    totalConfidence += maxProb
                    charCount++
                }
            }
            prevClass = maxClass
        }

        val avgConfidence = if (charCount > 0) totalConfidence / charCount else 0f
        return Pair(sb.toString(), avgConfidence)
    }

    /**
     * Get the model file path for a given tier.
     */
    private fun getModelFile(tier: ModelTier): File? {
        val filename = when (tier) {
            ModelTier.LITE -> "htr-lite.onnx"
            ModelTier.FULL -> "htr-full.onnx"
        }
        val file = File(context.filesDir, "models/$filename")
        return if (file.exists()) file else null
    }

    /**
     * Check if a model file exists for the given tier.
     */
    fun isModelAvailable(tier: ModelTier): Boolean = getModelFile(tier) != null

    /**
     * Release resources.
     */
    fun close() {
        ortSession?.close()
        ortEnv?.close()
        ortSession = null
        ortEnv = null
        modelLoaded = false
    }
}
