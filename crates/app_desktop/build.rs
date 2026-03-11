fn main() {
    // Embed icon into Windows executable (shows in taskbar, file explorer, Alt+Tab)
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "Voxlink");
        res.set("FileDescription", "Voxlink - Voice Without Limits");
        res.compile().expect("Failed to compile Windows resources");
    }
}
