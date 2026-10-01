package com.grafium.app

import android.content.Context
import org.json.JSONObject
import java.io.File

/** Validation-only JNI adapter. It cannot prepare, mutate, or advance narration. */
internal object ReaderOfflineSpeech {
  private val nativeLibrary by lazy { System.loadLibrary("grafium_lib") }

  private external fun nativeVoice(root: String, command: String, args: String): String

  internal fun validationResult(json: String): JSONObject {
    val response = JSONObject(json)
    if (response.has("error")) {
      throw IllegalArgumentException("VOICE_PACKAGE_INVALID: ${response.getString("error")}")
    }
    return response.getJSONObject("value")
  }

  fun validatePackage(context: Context, directory: File, manifest: JSONObject): JSONObject {
    val root = File(context.noBackupFilesDir, "private-reader/voices")
    check(root.isDirectory || root.mkdirs()) { "VOICE_STORAGE_UNAVAILABLE" }
    try {
      nativeLibrary
      return validationResult(nativeVoice(root.absolutePath, "validatePackage",
        JSONObject().put("directory", directory.absolutePath).put("manifest", manifest).toString()))
    } catch (error: LinkageError) {
      throw IllegalStateException(
        "VOICE_VALIDATION_UNAVAILABLE: shared native package validation must load before installing or using a voice", error)
    }
  }
}
