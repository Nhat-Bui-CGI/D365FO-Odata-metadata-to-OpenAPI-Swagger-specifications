fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .unwrap();
    // Embed the app icon in the Windows .exe.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../d365-openapi-tauri/icons/icon.ico")
            .compile()
            .expect("embed Windows icon");
    }
}
