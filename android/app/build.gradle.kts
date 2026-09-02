import java.util.Properties

plugins {
    id("com.android.application")
}

// ---- Load local.properties (sdk.dir, ndk.dir) ----
val localProps = Properties().apply {
    val f = rootProject.file("local.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
val ndkDir: String = localProps.getProperty("ndk.dir")
    ?: System.getenv("ANDROID_NDK_HOME")
    ?: error("Set ndk.dir in local.properties or ANDROID_NDK_HOME env var")

// ---- Game Directory ----

val engineDir = rootProject.projectDir.parentFile  // engine/

val gameDir = if (project.hasProperty("gameDir")) {
    file(project.property("gameDir") as String)
} else {
    engineDir.parentFile  // Assume game project is parent of engine/
}

// ---- Game Config (android.toml) ----
// Read flat key = "value" pairs from the game's android.toml.
// This lets each game project customize its APK without engine changes.

val gameConfig = mutableMapOf<String, String>()
val configFile = gameDir.resolve("android.toml")
if (configFile.exists()) {
    configFile.readLines().forEach { line ->
        val trimmed = line.trim()
        if (trimmed.startsWith("#") || trimmed.isEmpty()) return@forEach
        // Match: key = "string value"
        val strMatch = Regex("""^(\w+)\s*=\s*"([^"]*)"$""").find(trimmed)
        if (strMatch != null) {
            gameConfig[strMatch.groupValues[1]] = strMatch.groupValues[2]
            return@forEach
        }
        // Match: key = integer
        val intMatch = Regex("""^(\w+)\s*=\s*(\d+)$""").find(trimmed)
        if (intMatch != null) {
            gameConfig[intMatch.groupValues[1]] = intMatch.groupValues[2]
        }
    }
}

val cfgAppName = gameConfig["app_name"] ?: "Flint Game"
val cfgAppId = gameConfig["application_id"] ?: "com.flint.game"
val cfgOrientation = gameConfig["orientation"] ?: "landscape"
val cfgVersionName = gameConfig["version_name"] ?: "0.1.0"
val cfgVersionCode = gameConfig["version_code"]?.toIntOrNull() ?: 1
val cfgIconDir = gameConfig["icon_dir"] ?: ""

android {
    namespace = cfgAppId
    compileSdk = 35

    defaultConfig {
        applicationId = cfgAppId
        minSdk = 26          // AAudio (Kira audio) + Vulkan
        targetSdk = 34
        versionCode = cfgVersionCode
        versionName = cfgVersionName

        // Generate app_name as a string resource from config
        resValue("string", "app_name", cfgAppName)

        // Manifest placeholder for orientation
        manifestPlaceholders["screenOrientation"] = cfgOrientation

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

dependencies {
    implementation("androidx.games:games-activity:2.0.2")
    implementation("androidx.appcompat:appcompat:1.6.1")
}

// ---- Cargo NDK Build Task ----
// Builds the Rust cdylib for Android via cargo-ndk.
// The output .so files go into jniLibs/ for APK packaging.

tasks.register<Exec>("cargoNdkBuild") {
    description = "Build flint-android native library via cargo-ndk"
    workingDir = engineDir

    // Pass NDK path from local.properties so shell env vars aren't needed
    environment("ANDROID_NDK_HOME", ndkDir)

    val target = "arm64-v8a"
    val jniLibsDir = file("src/main/jniLibs")

    // Pass a unique build timestamp so the asset extractor always re-extracts.
    // This defeats Android Auto Backup restoring stale assets after reinstall.
    environment("FLINT_APK_VERSION", System.currentTimeMillis().toString())

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
        include("terrain/**")
        // Exclude engine internals and build artifacts
        exclude("engine/**")
        exclude("target/**")
        exclude("android/**")
        exclude("build/**")
        exclude(".git/**")
    }

    // Copy engine schemas
    from(engineDir.resolve("schemas")) {
        into("engine/schemas")
    }

    into(assetsDir)

    // Generate a manifest of all asset files for the Rust extractor.
    doLast {
        val manifest = File(assetsDir, "asset_manifest.txt")
        val lines = mutableListOf<String>()
        assetsDir.walkTopDown().filter { it.isFile && it.name != "asset_manifest.txt" }.forEach {
            lines.add(it.relativeTo(assetsDir).path.replace("\\", "/"))
        }
        manifest.writeText(lines.joinToString("\n"))
    }
}

// ---- Copy Game Icons Task ----

tasks.register<Copy>("copyGameIcons") {
    description = "Copy game launcher icons into Android res/"

    val iconSrcDir = if (cfgIconDir.isNotEmpty()) File(gameDir, cfgIconDir) else null

    // Only run if game specifies an icon directory and it exists
    enabled = iconSrcDir != null && iconSrcDir.exists()

    if (iconSrcDir != null && iconSrcDir.exists()) {
        from(iconSrcDir) {
            include("mipmap-*/ic_launcher.png")
        }
        into(file("src/main/res"))
    }
}

// ---- Copy APK to Game Directory Task ----

tasks.register<Copy>("copyApkToGame") {
    description = "Copy built APK to game project build/ directory"

    val apkFile = file("build/outputs/apk/debug/app-debug.apk")
    // Derive APK name from game name: lowercase, hyphens for spaces
    val apkName = cfgAppName.lowercase().replace(" ", "-") + ".apk"
    val outputDir = File(gameDir, "build")

    from(apkFile)
    into(outputDir)
    rename { apkName }
}

// Wire tasks as preBuild dependencies
tasks.named("preBuild") {
    dependsOn("cargoNdkBuild", "copyGameAssets", "copyGameIcons")
}

// Copy APK after assembly completes
tasks.whenTaskAdded {
    if (name == "assembleDebug" || name == "assembleRelease") {
        finalizedBy("copyApkToGame")
    }
}
