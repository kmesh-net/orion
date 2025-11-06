// Copyright 2025 The kmesh Authors
//
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
//

use glob::glob;
use std::path::{Path, PathBuf};

/// std::env::set_var("PROTOC", The Path of Protoc);
fn main() -> std::io::Result<()> {
    let descriptor_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("proto_descriptor.bin");

    let mut protos: Vec<PathBuf> = glob("data-plane-api/envoy/**/v3/*.proto").unwrap().filter_map(Result::ok).collect();

    let udpa_protos: Vec<PathBuf> = glob("xds/udpa/**/*.proto").unwrap().filter_map(Result::ok).collect();
    protos.extend(udpa_protos);

    let custom_protos: Vec<PathBuf> = glob("../proto/**/*.proto").unwrap().filter_map(Result::ok).collect();
    protos.extend(custom_protos);

    let include_paths = [
        "./data-plane-api/",
        "./xds/",
        "./protoc-gen-validate/",
        "./googleapis/",
        "./opencensus-proto/src/",
        "./opentelemetry-proto/",
        "./prometheus-client-model/",
        "./cel-spec/proto",
        "./protobuf/src/",
        "./udpa/udpa/type/v1",
        "../proto/",
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
    let include_paths = include_paths.into_iter().map(|p| Path::new(p).to_path_buf()).collect::<Vec<PathBuf>>();
    // Proceed w/ tonic_build
    tonic_prost_build::configure().build_server(true).build_client(true).compile_with_config(
        config,
        &protos,
        &include_paths,
    )
}
