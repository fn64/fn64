use core::fmt;

use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::Xxh3;

/// One fast 128-bit content identity for runtime-only comparisons.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FastContentDigest(u128);

impl FastContentDigest {
    pub fn hash(domain: &[u8], fields: &[&[u8]]) -> Self {
        let mut hash = Xxh3::new();
        hash.update(domain);
        for field in fields {
            hash.update(&(field.len() as u64).to_be_bytes());
            hash.update(field);
        }
        Self(hash.digest128())
    }

    pub const fn as_bytes(self) -> [u8; 16] {
        self.0.to_be_bytes()
    }
}

impl fmt::Debug for FastContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "xxh3-128:{:032x}", self.0)
    }
}

/// One SHA-256 content digest at a semantic boundary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn hash(domain: &[u8], fields: &[&[u8]]) -> Self {
        let mut hash = Sha256::new();
        hash.update(domain);
        for field in fields {
            hash.update((field.len() as u64).to_be_bytes());
            hash.update(field);
        }
        Self(hash.finalize().into())
    }

    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }

    pub const fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sha256:{self}")
    }
}

macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(ContentDigest);

        impl $name {
            pub(crate) const fn new(digest: ContentDigest) -> Self {
                Self(digest)
            }

            pub const fn digest(self) -> ContentDigest {
                self.0
            }

            pub const fn as_bytes(self) -> [u8; 32] {
                self.0.as_bytes()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

identity!(RawStreamIdentity);
identity!(JournalIdentity);
identity!(WorkloadIdentity);
identity!(RecordIdentity);
identity!(EffectIdentity);
identity!(GuestReadPlanIdentity);
identity!(GuestReadSetIdentity);
