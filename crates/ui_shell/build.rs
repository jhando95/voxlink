fn main() {
    // Slint's parser is recursive and can overflow Windows' default 1 MB
    // build-thread stack on larger UIs. Run the compile on a worker
    // thread with a generous 32 MB stack so the build is portable.
    let handle = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| slint_build::compile("ui/main.slint").unwrap())
        .expect("failed to spawn slint build thread");
    handle.join().expect("slint build thread panicked");
}
