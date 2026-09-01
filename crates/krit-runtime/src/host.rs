use std::{
    collections::BTreeMap,
    fmt,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use krit_capability::is_valid_resource_name;
use zeroize::Zeroize;

use crate::RuntimeError;

pub const MAX_HOST_INPUT_ENTRIES: usize = 256;

#[derive(Clone, Default)]
pub struct SecretStore {
    inner: Arc<BTreeMap<String, Arc<SecretBytes>>>,
}

#[derive(Clone, Default)]
pub struct HostInputs {
    config: Arc<BTreeMap<String, String>>,
    secrets: SecretStore,
    network_policy: NetworkPolicy,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkPolicy {
    allow_loopback: bool,
    allow_plaintext_bearer: bool,
}

pub(crate) struct SecretBytes(Vec<u8>);

impl SecretStore {
    pub fn new(values: BTreeMap<String, Vec<u8>>) -> Result<Self, RuntimeError> {
        if values.len() > MAX_HOST_INPUT_ENTRIES {
            return Err(RuntimeError::resource(format!(
                "host secrets exceed the {MAX_HOST_INPUT_ENTRIES}-entry limit"
            )));
        }
        let mut secrets = BTreeMap::new();
        for (name, bytes) in values {
            if !is_valid_resource_name(&name) {
                return Err(RuntimeError::setup(format!(
                    "invalid host secret name `{name}`"
                )));
            }
            secrets.insert(name, Arc::new(SecretBytes(bytes)));
        }
        Ok(Self {
            inner: Arc::new(secrets),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub(crate) fn get(&self, name: &str) -> Option<Arc<SecretBytes>> {
        self.inner.get(name).cloned()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, usize)> {
        self.inner
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.0.len()))
    }

    pub(crate) fn contains_exact_value(&self, value: &[u8]) -> bool {
        self.inner.values().any(|secret| secret.0 == value)
    }
}

impl HostInputs {
    pub fn new(
        config: BTreeMap<String, String>,
        secrets: SecretStore,
    ) -> Result<Self, RuntimeError> {
        if config.len() > MAX_HOST_INPUT_ENTRIES {
            return Err(RuntimeError::resource(format!(
                "host configuration exceeds the {MAX_HOST_INPUT_ENTRIES}-entry limit"
            )));
        }
        if let Some(name) = config.keys().find(|name| !is_valid_resource_name(name)) {
            return Err(RuntimeError::setup(format!(
                "invalid host configuration key `{name}`"
            )));
        }
        Ok(Self {
            config: Arc::new(config),
            secrets,
            network_policy: NetworkPolicy::default(),
        })
    }

    pub fn with_network_policy(mut self, policy: NetworkPolicy) -> Self {
        self.network_policy = policy;
        self
    }

    pub(crate) fn config(&self) -> &BTreeMap<String, String> {
        &self.config
    }

    pub(crate) const fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    pub(crate) const fn network_policy(&self) -> NetworkPolicy {
        self.network_policy
    }
}

impl NetworkPolicy {
    pub const fn loopback_for_tests() -> Self {
        Self {
            allow_loopback: true,
            allow_plaintext_bearer: false,
        }
    }

    pub const fn with_plaintext_bearer_for_tests(mut self) -> Self {
        self.allow_plaintext_bearer = true;
        self
    }

    pub(crate) const fn permits_plaintext_bearer(self) -> bool {
        self.allow_plaintext_bearer
    }

    pub(crate) fn permits_address(self, address: IpAddr) -> bool {
        match address {
            IpAddr::V4(address) => self.permits_v4(address),
            IpAddr::V6(address) => {
                if let Some(mapped) = address.to_ipv4() {
                    return self.permits_v4(mapped);
                }
                if address.is_unspecified()
                    || address.is_multicast()
                    || address.is_unique_local()
                    || address.is_unicast_link_local()
                {
                    return false;
                }
                if address.is_loopback() {
                    return self.allow_loopback;
                }
                let segments = address.segments();
                let globally_allocated = segments[0] & 0xe000 == 0x2000;
                let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
                let benchmarking = segments[0] == 0x2001 && segments[1] == 0x0002;
                let teredo = segments[0] == 0x2001 && segments[1] == 0;
                let six_to_four = segments[0] == 0x2002;
                let extended_documentation = segments[0] == 0x3fff && segments[1] & 0xf000 == 0;
                let orchid =
                    segments[0] == 0x2001 && matches!(segments[1] & 0xfff0, 0x0010 | 0x0020);
                globally_allocated
                    && !documentation
                    && !benchmarking
                    && !teredo
                    && !six_to_four
                    && !extended_documentation
                    && !orchid
            }
        }
    }

    fn permits_v4(self, address: Ipv4Addr) -> bool {
        let [first, second, third, _] = address.octets();
        if address.is_unspecified()
            || address.is_broadcast()
            || address.is_multicast()
            || address.is_private()
            || address.is_link_local()
            || first == 0
            || (first == 100 && second & 0xc0 == 64)
            || (first == 192 && second == 0 && third == 0)
            || (first == 192 && second == 0 && third == 2)
            || (first == 192 && second == 88 && third == 99)
            || (first == 198 && matches!(second, 18 | 19))
            || (first == 198 && second == 51 && third == 100)
            || (first == 203 && second == 0 && third == 113)
            || first >= 240
        {
            return false;
        }
        !address.is_loopback() || self.allow_loopback
    }
}

impl SecretBytes {
    pub(crate) fn expose_for_bearer(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretStore")
            .field("names", &self.inner.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for HostInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostInputs")
            .field("config_keys", &self.config.keys().collect::<Vec<_>>())
            .field(
                "secret_names",
                &self.secrets.inner.keys().collect::<Vec<_>>(),
            )
            .field("network_policy", &self.network_policy)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::NetworkPolicy;

    #[test]
    fn default_network_policy_allows_only_public_unicast_addresses() {
        let policy = NetworkPolicy::default();
        for address in [
            Ipv4Addr::new(0, 1, 2, 3),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(169, 254, 169, 254),
            Ipv4Addr::new(192, 0, 0, 1),
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(192, 88, 99, 1),
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(198, 51, 100, 1),
            Ipv4Addr::new(203, 0, 113, 1),
            Ipv4Addr::new(224, 0, 0, 1),
            Ipv4Addr::new(240, 0, 0, 1),
        ] {
            assert!(!policy.permits_address(IpAddr::V4(address)), "{address}");
        }
        for address in [
            Ipv6Addr::LOCALHOST,
            "fc00::1".parse().expect("test address should parse"),
            "fe80::1".parse().expect("test address should parse"),
            "2001:db8::1".parse().expect("test address should parse"),
            "2001:2::1".parse().expect("test address should parse"),
            "2001:20::1".parse().expect("test address should parse"),
            "2001::1".parse().expect("test address should parse"),
            "2002:7f00:1::1".parse().expect("test address should parse"),
            "3fff::1".parse().expect("test address should parse"),
        ] {
            assert!(!policy.permits_address(IpAddr::V6(address)), "{address}");
        }
        assert!(policy.permits_address(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(
            policy.permits_address(IpAddr::V6(
                "2606:4700:4700::1111"
                    .parse()
                    .expect("test address should parse")
            ))
        );
        assert!(
            NetworkPolicy::loopback_for_tests().permits_address(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
    }
}
