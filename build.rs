extern crate tower_grpc_build;

fn main() {
    // Generate a client from rkt proto definition
    tower_grpc_build::Config::new()
        .enable_server(false)
        .enable_client(true)
        .build(&["proto/v1alpha.proto"], &["proto/"])
        .unwrap_or_else(|e| panic!("protobuf compilation failed: {}", e));
}
