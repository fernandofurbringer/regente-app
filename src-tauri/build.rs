fn main() {
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The lib's own test harness imports comctl32 TaskDialogIndirect (menus and tray pull it in), which exists only
        // in Common Controls v6. The app binary gets that manifest from tauri-build. This dependency is added to the
        // linker's generated manifest (no second manifest resource, which would clash with tauri's); without it the
        // lib tests do not start (STATUS_ENTRYPOINT_NOT_FOUND).
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
