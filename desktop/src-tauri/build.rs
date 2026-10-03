fn main() {
    // Build identity so a running app can be matched to the source it came from.
    let id = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "nogit".into());
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=NOKI_BUILD_ID={id}-{ts}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=src/photos_bridge.m");
    cc::Build::new()
        .file("src/photos_bridge.m")
        .flag("-fobjc-arc")
        .compile("noki_photos_bridge");
    println!("cargo:rustc-link-lib=framework=Photos");
    println!("cargo:rustc-link-lib=framework=AppKit");
    // Die Oberflaechen werden in die Binaerdatei eingebettet. Ohne diese
    // Zeilen blieb eine geaenderte ask.html/index.html im alten Stand
    // stecken: der Build lief durch, das Programm zeigte die alte Seite.
    for datei in [
        "../index.html",
        "../ask.html",
        "../kompakt.html",
        "../markdown.js",
        "../intelligence.js",
        "../intelligence.css",
        "../code-view.js",
        "../code-view.css",
        "tauri.conf.json",
    ] {
        println!("cargo:rerun-if-changed={datei}");
    }
    tauri_build::build()
}
