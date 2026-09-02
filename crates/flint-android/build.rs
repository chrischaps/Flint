fn main() {
    // Re-run this build script (and thus recompile the crate) whenever the
    // FLINT_APK_VERSION environment variable changes.  Gradle sets this to a
    // timestamp so the asset extractor's version marker never matches a stale
    // backup, forcing re-extraction of APK assets on every install.
    println!("cargo:rerun-if-env-changed=FLINT_APK_VERSION");
}
