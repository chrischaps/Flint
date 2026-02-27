plugins {
    id("com.android.application")
}

android {
    namespace = "com.flint.game"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.flint.game"
        minSdk = 26          // AAudio (Kira audio) + Vulkan
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    sourceSets {
        getByName("main") {
            // Native .so files built by cargo-ndk
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

// No Java dependencies needed — NativeActivity is built into Android.

// ---- Cargo NDK Build Task ----
// Builds the Rust cdylib for Android via cargo-ndk.
// The output .so files go into jniLibs/ for APK packaging.

val engineDir = rootProject.projectDir.parentFile  // engine/

tasks.register<Exec>("cargoNdkBuild") {
    description = "Build flint-android native library via cargo-ndk"
    workingDir = engineDir

    val target = "arm64-v8a"
    val jniLibsDir = file("src/main/jniLibs")

    commandLine(
        "cargo", "ndk",
        "-t", target,
        "--platform", "26",
        "-o", jniLibsDir.absolutePath,
        "build", "--release",
        "-p", "flint-android"
    )
}

// ---- Copy Game Assets Task ----
// Copies the game project's scene files, schemas, textures, scripts, and audio
// into the APK's assets/ directory. Pass -PgameDir=/path/to/game to specify
// the game project root (defaults to the engine's parent directory).

val gameDir = if (project.hasProperty("gameDir")) {
    file(project.property("gameDir") as String)
} else {
    engineDir.parentFile  // Assume game project is parent of engine/
}

tasks.register<Copy>("copyGameAssets") {
    description = "Copy game assets into APK assets directory"
    val assetsDir = file("src/main/assets")

    // Clean previous assets
    doFirst {
        assetsDir.deleteRecursively()
        assetsDir.mkdirs()
    }

    // Copy game-level files
    from(gameDir) {
        include("**/*.toml")
        include("schemas/**")
        include("scripts/**")
        include("sprites/**")
        include("textures/**")
        include("models/**")
        include("audio/**")
        include("animations/**")
        // Exclude engine internals and build artifacts
        exclude("engine/**")
        exclude("target/**")
        exclude("android/**")
        exclude(".git/**")
    }

    // Copy engine schemas
    from(engineDir.resolve("schemas")) {
        into("engine/schemas")
    }

    into(assetsDir)

    // Generate a manifest of all asset files for the Rust extractor.
    // Android's NDK AssetDir only enumerates files (not subdirectories),
    // so we write a flat list of relative paths the extractor can read.
    doLast {
        val manifest = File(assetsDir, "asset_manifest.txt")
        val lines = mutableListOf<String>()
        assetsDir.walkTopDown().filter { it.isFile && it.name != "asset_manifest.txt" }.forEach {
            lines.add(it.relativeTo(assetsDir).path.replace("\\", "/"))
        }
        manifest.writeText(lines.joinToString("\n"))
    }
}

// Wire both tasks as preBuild dependencies
tasks.named("preBuild") {
    dependsOn("cargoNdkBuild", "copyGameAssets")
}
