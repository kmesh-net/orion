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

use envoy_data_plane_api::{
    google::protobuf::Any,
    prost::{DecodeError, Message, Name},
    prost_reflect::{DescriptorPool, DynamicMessage},
};
use serde::de::Error;

#[derive(Debug, thiserror::Error)]
pub enum DecodeAnyError {
    #[error("Failed to decode protobuf extension type({0})")]
    ProtobufError(&'static str, #[source] DecodeError),
    #[error("Failed to decode yaml extension type({0})")]
    YamlError(#[from] serde_yaml::Error),
}

/// Decode yaml string as a protobuf message
///
/// Note: this is potentially expensive in that it will parse the protobuf
/// descriptor before actually parsing the yaml. A faster alternative would
///  cache this step in a lazy cell
pub fn from_serde_deserializer<'de, T, D>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Message + Name + Default,
{
    let pool = DescriptorPool::decode(envoy_data_plane_api::FILE_DESCRIPTOR_SET_BYTES).map_err(D::Error::custom)?;
    let name = T::full_name();
    let message_descriptor = pool.get_message_by_name(&name).ok_or(D::Error::custom(name))?;
    let dynmsg = DynamicMessage::deserialize(message_descriptor, deserializer)?;
    dynmsg.transcode_to().map_err(D::Error::custom)
}

pub fn from_yaml<T>(inp: &str) -> Result<T, DecodeAnyError>
where
    T: Message + Name + Default,
{
    let deserializer = serde_yaml::Deserializer::from_str(inp);
    from_serde_deserializer(deserializer).map_err(Into::into)
}

/// Decode payload from Any type into a generic return type.
///
/// - err_desc is used in the returned error message.
///
/// .type_url is not checked, it is up to the caller to verify it
/// matches the output type
pub fn decode_any_type<R>(any: &Any, err_desc: &'static str) -> Result<R, DecodeAnyError>
where
    R: Message + Default,
{
    R::decode(any.value.as_slice()).map_err(|e| DecodeAnyError::ProtobufError(err_desc, e))
}

#[cfg(test)]
mod tests {
    use super::{Any, decode_any_type, from_yaml};
    use envoy_data_plane_api::{
        envoy::{
            config::listener::v3::{Filter, FilterChain, Listener, filter::ConfigType},
            extensions::filters::network::http_connection_manager::v3::HttpConnectionManager,
        },
        prost::Message,
    };

    fn expected_conn_manager() -> HttpConnectionManager {
        HttpConnectionManager { server_name: "name".to_owned(), ..Default::default() }
    }

    /// In a previous version there was a different impl of the Any type was
    /// used that would be deserialized directly from yaml. This introduced a bug
    /// where depending on the source the decoding would differ. This is no longer
    /// the case, so the asserts in this test were inverted (but the comments remain).
    #[test]
    fn protobuf_decode_any_conn_manager() {
        // protobuf - Message TestInner { string name = 10; }
        // same as the envoy HttpConnectionManager server_name
        // (api/envoy/extensions/filters/network/http_connection_manager/v3/http_connection_manager.proto)
        const PAYLOAD: &[u8] = b"R\x04name";
        let v = Any { type_url: "url".into(), value: PAYLOAD.to_vec() };
        let m: HttpConnectionManager = decode_any_type(&v, "---").unwrap();
        assert_eq!(m, expected_conn_manager());

        // Counter proof
        let y: Result<HttpConnectionManager, _> = decode_any_type(&v, "---");
        //assert!(y.is_err(), "Yaml decoder cannot handle protobuf data");
        assert!(y.is_ok());
    }

    const YAML_PAYLOAD_LISTEN_FILTER: &str = r#"
name: name
filterChains:
  - filters:
      - name: envoy.filters.network.http_connection_manager
        typedConfig:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          server_name: name
            "#;

    fn prost_payload_listen_filter() -> Vec<u8> {
        let http_man = HttpConnectionManager { server_name: "name".to_owned(), ..Default::default() };

        let any = Any {
            type_url:
                "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
                    .to_string(),
            value: http_man.encode_to_vec(),
        };

        let filter = Filter {
            name: "envoy.filters.network.http_connection_manager".to_owned(),
            config_type: Some(ConfigType::TypedConfig(any)),
        };

        let fc = FilterChain { filters: vec![filter], ..Default::default() };

        Listener { name: "name".to_owned(), filter_chains: vec![fc], ..Default::default() }.encode_to_vec()
    }

    /// In a previous version there was a different impl of the Any type was
    /// used that would be deserialized directly from yaml. This introduced a bug
    /// where depending on the source the decoding would differ. This is no longer
    /// the case, so the asserts in this test were inverted (but the comments remain).
    #[test]
    #[allow(clippy::panic)]
    fn yaml_and_prost_eq() {
        let l_yaml: Listener = from_yaml(YAML_PAYLOAD_LISTEN_FILTER).unwrap();
        let l_pb: Listener = Listener::decode(prost_payload_listen_filter().as_slice()).unwrap();

        //assert_ne!(l_yaml, l_pb, "yaml decoded obj is different from prost obj");
        assert_eq!(l_yaml, l_pb);

        {
            let mut l_yaml2 = l_yaml.clone();
            l_yaml2.filter_chains[0].filters[0].config_type = l_pb.filter_chains[0].filters[0].config_type.clone();
            assert_eq!(l_yaml2, l_pb, "Difference is the any type");
        }

        let ConfigType::TypedConfig(any_yaml) = l_yaml.filter_chains[0].filters[0].config_type.as_ref().unwrap() else {
            panic!("Expecting TypedConfig in yaml");
        };
        let ConfigType::TypedConfig(any_pb) = l_pb.filter_chains[0].filters[0].config_type.as_ref().unwrap() else {
            panic!("Expecting TypedConfig in pb");
        };

        assert_eq!(any_yaml.type_url, any_pb.type_url, "Any type urls are the same");

        let man_yaml: HttpConnectionManager = decode_any_type(any_yaml, "").unwrap();
        let man_pb: HttpConnectionManager = decode_any_type(any_pb, "").unwrap();
        assert_eq!(man_yaml, man_pb, "Both http managers are identical after decoding QED");

        let wrong_pb: HttpConnectionManager = decode_any_type(any_yaml, "").unwrap();
        //assert_ne!(
        //    wrong_pb, man_yaml,
        //    "Decoding yaml any via prost works, but is wrong"
        //);
        assert_eq!(wrong_pb, man_yaml);

        let wrong_yaml: Result<HttpConnectionManager, _> = decode_any_type(any_pb, "");
        //assert!(
        //    wrong_yaml.is_err(),
        //    "Decoding prost any via serde_yaml fails"
        //);
        assert!(wrong_yaml.is_ok());
    }

    #[test]
    fn decode_errors() {
        const BAD_EXT: &str = r#"
name: name
filterChains:
  - filters:
      - name: envoy.filters.network.http_connection_manager
        typedConfig:
          "@type": type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager
          server_name2: name
            "#;

        from_yaml::<Listener>(BAD_EXT).unwrap_err();
    }
}
