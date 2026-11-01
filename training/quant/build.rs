fn main() {
    // A pure Rust protobuf compile, so that no protoc binary is needed.
    let descriptors = protox::compile(["onnx.proto"], ["proto"]).expect("the ONNX schema compiles");
    prost_build::Config::new().compile_fds(descriptors).expect("the ONNX types generate");
}
