import com.android.build.api.artifact.SingleArtifact
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.serialization)
}

// gms flavor: google-services is applied only when its config file is present.
val gmsConfig = file("src/gms/google-services.json")
val hasGmsConfig = gmsConfig.exists()
if (hasGmsConfig) {
    apply(plugin = libs.plugins.google.services.get().pluginId)
}

// Release signing: keystore.properties next to settings.gradle.kts, else environment variables.
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
fun signingValue(key: String, env: String): String? =
    keystoreProps.getProperty(key) ?: providers.environmentVariable(env).orNull

val storeFilePath = signingValue("storeFile", "RUSTYBEE_STORE_FILE")

// The repository-root VERSION file is the single version source (spec 001).
val appVersion = rootProject.file("../VERSION").readText().trim()
val appVersionCode = appVersion.split(".").map(String::toInt).let { (major, minor, patch) ->
    major * 10_000 + minor * 100 + patch
}

android {
    namespace = "dev.joowdx.rustybee"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.joowdx.rustybee"
        minSdk = 36
        targetSdk = 36
        versionCode = appVersionCode
        versionName = appVersion
    }

    buildFeatures {
        buildConfig = true
    }

    flavorDimensions += "dist"
    productFlavors {
        create("foss") {
            dimension = "dist"
            isDefault = true
        }
        create("gms") {
            dimension = "dist"
        }
    }

    signingConfigs {
        if (storeFilePath != null) {
            create("release") {
                storeFile = rootProject.file(storeFilePath)
                storePassword = signingValue("storePassword", "RUSTYBEE_STORE_PASSWORD")
                keyAlias = signingValue("keyAlias", "RUSTYBEE_KEY_ALIAS")
                keyPassword = signingValue("keyPassword", "RUSTYBEE_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("test").resources.directories.add("../../protocol/fixtures")
    }

    lint {
        abortOnError = true
        checkReleaseBuilds = false
    }

    packaging {
        resources.excludes += setOf("META-INF/*.version", "META-INF/**/LICENSE*", "kotlin/**", "DebugProbesKt.bin")
    }
}

dependencies {
    implementation(libs.okhttp)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.zxing.core)
    "gmsImplementation"(libs.firebase.messaging)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}

// AC-012-07: unit tests read the merged manifest of their own variant.
androidComponents {
    onVariants { variant ->
        val manifest = variant.artifacts.get(SingleArtifact.MERGED_MANIFEST)
        val testTask = "test${variant.name.replaceFirstChar { it.uppercase() }}UnitTest"
        tasks.withType<Test>().matching { it.name == testTask }.configureEach {
            inputs.file(manifest).withPropertyName("mergedManifest")
            jvmArgumentProviders.add(CommandLineArgumentProvider {
                listOf("-Drustybee.mergedManifest=${manifest.get().asFile.absolutePath}")
            })
        }
    }
}

if (!hasGmsConfig) {
    val message = "The gms flavor needs app/src/gms/google-services.json from your Firebase project. " +
        "Add it, or build the foss flavor (e.g. ./gradlew assembleFossRelease)."
    tasks.matching { it.name.startsWith("pre") && it.name.contains("Gms") && it.name.endsWith("Build") }
        .configureEach { doFirst { throw GradleException(message) } }
}
