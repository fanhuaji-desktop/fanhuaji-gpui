fn main() {
    // The Windows exe carries its icon as a resource; macOS gets it from the bundle.
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .compile()
        .expect("embed the Windows icon");
}
