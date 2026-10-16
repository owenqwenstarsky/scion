use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// 32-byte content hash. Domain separation is applied by the constructor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash32(pub [u8; 32]);

impl Hash32 {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn parse_hex(s: &str) -> Result<Self> {
        let raw = hex::decode(s).map_err(|e| Error::Invalid(format!("hash: {e}")))?;
        let bytes: [u8; 32] = raw
            .try_into()
            .map_err(|_| Error::Invalid("hash must be 32 bytes".into()))?;
        Ok(Self(bytes))
    }
}

impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash32({})", &self.to_hex()[..12])
    }
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

fn hash_domain(domain: &str, data: &[u8]) -> Hash32 {
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    hasher.update(data);
    Hash32(*hasher.finalize().as_bytes())
}

macro_rules! id_type {
    ($name:ident, $domain:literal) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Hash32);

        impl $name {
            pub const DOMAIN: &'static str = $domain;

            pub fn from_hash(hash: Hash32) -> Self {
                Self(hash)
            }

            pub fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(Hash32(bytes))
            }

            pub fn of(data: &[u8]) -> Self {
                Self(hash_domain($domain, data))
            }

            pub fn as_bytes(&self) -> &[u8; 32] {
                self.0.as_bytes()
            }

            pub fn to_hex(&self) -> String {
                self.0.to_hex()
            }

            pub fn parse_hex(s: &str) -> Result<Self> {
                Ok(Self(Hash32::parse_hex(s)?))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), &self.to_hex()[..12])
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                $name::parse_hex(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

id_type!(BlobId, "scion blob v1");
id_type!(ChunkId, "scion chunk v1");
id_type!(TreeId, "scion tree v1");
id_type!(ChangesetId, "scion changeset v1");

/// Hash an opaque token (session or API token). Not a content id.
pub fn hash_token(domain: &str, token: &[u8]) -> Hash32 {
    hash_domain(domain, token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_do_not_collide() {
        let data = b"same bytes";
        assert_ne!(*BlobId::of(data).as_bytes(), *ChunkId::of(data).as_bytes());
        assert_ne!(*BlobId::of(data).as_bytes(), *TreeId::of(data).as_bytes());
        assert_ne!(
            *BlobId::of(data).as_bytes(),
            *ChangesetId::of(data).as_bytes()
        );
    }
}
