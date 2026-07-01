plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.grafium.companion"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.grafium.companion"
        minSdk = 26
        targetSdk = 35
        versionCode = 2
        versionName = "0.2.0"
    }

    buildFeatures {
        viewBinding = true
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("com.google.android.material:material:1.12.0")

    // Low-latency stylus rendering (front-buffer rendering)
    implementation("androidx.graphics:graphics-core:1.0.2")

    // Motion prediction for stylus (reduces perceived latency)
    implementation("androidx.input:input-motionprediction:1.0.0-beta05")

    // ONNX Runtime for on-device HTR inference + training
    // The training artifact bundles the full inference runtime, so we only need one.
    implementation("com.microsoft.onnxruntime:onnxruntime-training-android:1.19.0")

    // Coroutines for background HTR processing
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
}
