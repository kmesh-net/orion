use std::path::PathBuf;

use glob::glob;

/// std::env::set_var("PROTOC", The Path of Protoc);
fn main() -> std::io::Result<()> {
    let descriptor_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("proto_descriptor.bin");
    let protos: Vec<PathBuf> = glob("data-plane-api/envoy/**/v3/*.proto").unwrap().filter_map(Result::ok).collect();

    let include_paths = [
        "data-plane-api/",
        "xds/",
        "protoc-gen-validate/",
        "googleapis/",
        "opencensus-proto/src/",
        "opentelemetry-proto/",
        "prometheus-client-model/",
        "cel-spec/proto",
        "protobuf/src/",
    ];

    let mut config = prost_build::Config::new();
    config
        .file_descriptor_set_path(descriptor_path.clone())
        .enable_type_names()
        .compile_well_known_types()
        .include_file("mod.rs");

    // this is the same as prost_reflect_build::Builder::configure but we
    // cannot use it due to different versions of prost_build in dependencies
    //
    // This requires our crate to provide FILE_DESCRIPTOR_SET_BYTES an include bytes
    // of descriptor_path
    config.compile_protos(&protos, &include_paths)?;
    let pool_attribute = r#"#[prost_reflect(file_descriptor_set_bytes = "crate::FILE_DESCRIPTOR_SET_BYTES")]"#;

    let buf = std::fs::read(&descriptor_path)?;
    let descriptor = prost_reflect::DescriptorPool::decode(buf.as_ref()).expect("Invalid file descriptor");
    for message in descriptor.all_messages() {
        let full_name = message.full_name();
        config
            .type_attribute(full_name, "#[derive(::prost_reflect::ReflectMessage)]")
            .type_attribute(full_name, format!(r#"#[prost_reflect(message_name = "{}")]"#, full_name,))
            .type_attribute(full_name, pool_attribute);
    }

    // Proceed w/ tonic_build
    tonic_build::configure().build_server(true).build_client(true).compile_protos_with_config(
        config,
        &protos,
        &include_paths,
    )?;

    Ok(())
}
