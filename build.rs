//! Compiles the gRPC control protocol. Uses a vendored `protoc` so CI needs no
//! `protobuf-compiler` installed.

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/control.proto"], &["proto"])
        .expect("failed to compile proto/control.proto");

    println!("cargo:rerun-if-changed=proto/control.proto");

    // The embedded frontend (`static/`) is baked into the binary by
    // rust-embed at compile time. cargo does not track those files, so a
    // frontend-only change would silently keep serving the old UI — rerun the
    // build script (which marks the crate dirty) whenever `static/` changes.
    println!("cargo:rerun-if-changed=static");
}
