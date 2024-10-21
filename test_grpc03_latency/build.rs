fn main() -> Result<(), Box<dyn std::error::Error>> {
    let no_includes: &[&str] = &[];
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&["proto/helloworld.proto"], no_includes)?;
    Ok(())
}
