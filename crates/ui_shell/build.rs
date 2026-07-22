fn main() {
    // Slint's parser is recursive and can overflow Windows' default 1 MB
    // build-thread stack on larger UIs. Run the compile on a worker
    // thread with a generous 32 MB stack so the build is portable.
    let handle = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            // Pin the std-widget style to fluent-dark on every platform: the
            // few remaining std widgets (TextEdit composer, scrollbars) must
            // match the graphite theme instead of platform-native light chrome.
            let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
            slint_build::compile_with_config("ui/main.slint", config).unwrap()
        })
        .expect("failed to spawn slint build thread");
    handle.join().expect("slint build thread panicked");
}
