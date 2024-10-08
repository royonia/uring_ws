use std::path::PathBuf;

fn main() {
    // Tell cargo to look for the shared library in the `liburing` directory
    let liburing_path = PathBuf::from("/home/roytang/github/liburing/src");

    // Set the library search path for the linker to find liburing.so
    println!("cargo:rustc-link-search=native={}", liburing_path.display());

    // Link the shared library `uring` (which corresponds to liburing.so)
    println!("cargo:rustc-link-lib=dylib=uring");

    // Tell Cargo to re-run this script if any files in the liburing directory change
    //println!("cargo:rerun-if-changed=../path/to/liburing");
}
