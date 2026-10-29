fn main() {
    // Embed the app icon in the Windows .exe.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../d365-openapi-tauri/icons/icon.ico")
            .compile()
            .expect("embed Windows icon");
    }
}
