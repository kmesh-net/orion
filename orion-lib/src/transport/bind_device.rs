// SPDX-FileCopyrightText: © 2025 kmesh authors
// SPDX-License-Identifier: Apache-2.0
//
// Copyright 2025 kmesh authors
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

pub use orion_configuration::config::transport::BindDevice;

#[cfg(target_os = "linux")]
pub(crate) fn bind_device(s: &tokio::net::TcpSocket, binddev: &BindDevice) -> std::io::Result<()> {
    let name = binddev.interface();
    tracing::trace!("binding socket to dev {:?}", name);
    s.bind_device(Some(name.to_bytes_with_nul()))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn bind_device(_: &tokio::net::TcpSocket, _: &BindDevice) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Other, "BINDTODEVICE is not supported"))
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;
    use orion_data_plane_api::envoy_data_plane_api::envoy::config::core::v3::{socket_option, SocketOption};

    #[test]
    fn envoy_bind_device_none() {
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            value: None,
            ..Default::default()
        };
        BindDevice::try_from(opt).unwrap_err();
    }

    #[test]
    fn envoy_bind_device_eth0_no_null() {
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            value: Some(socket_option::Value::BufValue("eth0".as_bytes().to_vec())),
            ..Default::default()
        };
        let bind = BindDevice::try_from(opt).unwrap();
        let expected = "eth0".parse().unwrap();
        assert_eq!(bind, expected);
    }

    #[test]
    fn envoy_bind_device_eth0_null() {
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            value: Some(socket_option::Value::BufValue(b"eth0\0".to_vec())),
            ..Default::default()
        };
        let bind = BindDevice::try_from(opt).unwrap();
        let expected = "eth0".parse().unwrap();
        assert_eq!(bind, expected);
    }

    #[test]
    fn envoy_bind_device_bad_option() {
        let opt =
            SocketOption { description: String::new(), level: 1, name: 1, state: 0, value: None, ..Default::default() };
        BindDevice::try_from(opt).unwrap_err();
    }

    #[test]
    fn envoy_bind_device_too_long() {
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            value: Some(socket_option::Value::BufValue(b"0123456789ABCDEF".to_vec())),
            ..Default::default()
        };
        BindDevice::try_from(opt).unwrap_err();
    }

    #[test]
    fn envoy_bind_invalid_null() {
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            value: Some(socket_option::Value::BufValue(b"a\0b".to_vec())),
            ..Default::default()
        };
        BindDevice::try_from(opt).unwrap_err();
    }

    #[test]
    fn roundtrip_valid_binary() {
        let iface = CStr::from_bytes_with_nul(b"a\x1b\0").unwrap();
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            // example of a valid string taken from
            // https://unix.stackexchange.com/a/677481
            // note that spaces etc. are still dissallowed by linux but accepted here
            // but that's not a huge issue, binding will just fail. What is important is that we accept
            // binary values and round-trip them correctly
            value: Some(socket_option::Value::BufValue(iface.to_bytes().to_owned())),
            ..Default::default()
        };
        let bd = BindDevice::try_from(opt).unwrap();
        assert_eq!(bd.interface(), iface);
        let ng_string = serde_yaml::to_string(&bd).unwrap();
        let bd: BindDevice = serde_yaml::from_str(&ng_string).unwrap();
        assert_eq!(iface, bd.interface())
    }

    #[test]
    fn roundtrip_valid_cstr() {
        let iface = CStr::from_bytes_with_nul(b"eth0\0").unwrap();
        let opt = SocketOption {
            description: String::new(),
            level: 1,
            name: 25,
            state: 0,
            value: Some(socket_option::Value::BufValue(iface.to_bytes().to_owned())),
            ..Default::default()
        };
        let bd = BindDevice::try_from(opt).unwrap();
        assert_eq!(bd.interface(), iface);
        let ng_string = serde_yaml::to_string(&bd).unwrap();
        let bd: BindDevice = serde_yaml::from_str(&ng_string).unwrap();
        assert_eq!(iface, bd.interface())
    }

    #[test]
    fn direct_decode_bytes() {
        let yaml = "interface_bytes: YRs=";
        let iface = CStr::from_bytes_with_nul(b"a\x1b\0").unwrap();
        let bd: BindDevice = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(iface, bd.interface())
    }

    #[test]
    fn direct_decode_iface() {
        let yaml = "interface: eth0";
        let iface = CStr::from_bytes_with_nul(b"eth0\0").unwrap();
        let bd: BindDevice = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(iface, bd.interface())
    }
}
