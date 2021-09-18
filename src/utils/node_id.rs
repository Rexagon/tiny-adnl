use std::convert::{TryFrom, TryInto};

use anyhow::Result;
use rand::Rng;
use ton_api::{ton, IntoBoxed};

use super::tl_view::PublicKeyView;
use super::{hash, serialize, serialize_boxed};

// todo return Eq, PartialEq
#[derive(Debug, Copy, Clone)]
pub struct AdnlNodeIdFull(ed25519_consensus::VerificationKey);

impl AdnlNodeIdFull {
    pub fn new(public_key: ed25519_consensus::VerificationKey) -> Self {
        Self(public_key)
    }

    pub fn public_key(&self) -> &ed25519_consensus::VerificationKey {
        &self.0
    }

    pub fn as_tl(&self) -> ton::pub_::publickey::Ed25519 {
        ton::pub_::publickey::Ed25519 {
            key: ton::int256(self.0.as_ref().try_into().unwrap()),
        }
    }

    pub fn verify(&self, message: &[u8], other_signature: &[u8]) -> Result<()> {
        let other_signature = ed25519_consensus::Signature::try_from(other_signature)?;
        self.0.verify(&other_signature, message)?;
        Ok(())
    }

    pub fn verify_boxed<T, F>(&self, data: &T, extractor: F) -> Result<()>
    where
        T: IntoBoxed + Clone,
        F: FnOnce(&mut T) -> &mut ton::bytes,
    {
        let mut data = data.clone();
        let signature = std::mem::take(&mut extractor(&mut data).0);
        let buffer = serialize_boxed(data)?;
        self.verify(&buffer, &signature)
    }

    pub fn compute_short_id(&self) -> Result<AdnlNodeIdShort> {
        let hash = hash(self.as_tl())?;
        Ok(AdnlNodeIdShort::new(hash))
    }
}

impl std::fmt::Display for AdnlNodeIdFull {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

impl From<ed25519_consensus::VerificationKey> for AdnlNodeIdFull {
    fn from(key: ed25519_consensus::VerificationKey) -> Self {
        Self::new(key)
    }
}

impl TryFrom<&ton::PublicKey> for AdnlNodeIdFull {
    type Error = anyhow::Error;

    fn try_from(public_key: &ton::PublicKey) -> Result<Self> {
        match public_key {
            ton::PublicKey::Pub_Ed25519(public_key) => {
                let public_key =
                    ed25519_consensus::VerificationKeyBytes::from(public_key.key.0).try_into()?;
                Ok(Self::new(public_key))
            }
            _ => Err(AdnlNodeIdError::UnsupportedPublicKey.into()),
        }
    }
}

impl<'a> TryFrom<PublicKeyView<'a>> for AdnlNodeIdFull {
    type Error = anyhow::Error;

    fn try_from(value: PublicKeyView<'a>) -> Result<Self, Self::Error> {
        match value {
            PublicKeyView::Ed25519 { key } => {
                let public_key =
                    ed25519_consensus::VerificationKeyBytes::from(key.clone()).try_into()?;
                Ok(Self::new(public_key))
            }
            _ => Err(AdnlNodeIdError::UnsupportedPublicKey.into()),
        }
    }
}

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct AdnlNodeIdShort([u8; 32]);

impl AdnlNodeIdShort {
    pub fn new(hash: [u8; 32]) -> Self {
        Self(hash)
    }

    pub fn random() -> Self {
        Self(rand::thread_rng().gen())
    }

    pub fn as_slice(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn is_zero(&self) -> bool {
        for b in &self.0 {
            if b != &0 {
                return false;
            }
        }
        true
    }

    pub fn as_tl(&self) -> ton::adnl::id::short::Short {
        ton::adnl::id::short::Short {
            id: ton::int256(self.0),
        }
    }
}

impl std::fmt::Display for AdnlNodeIdShort {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

impl PartialEq<[u8]> for AdnlNodeIdShort {
    fn eq(&self, other: &[u8]) -> bool {
        self.0.eq(other)
    }
}

impl PartialEq<[u8; 32]> for AdnlNodeIdShort {
    fn eq(&self, other: &[u8; 32]) -> bool {
        self.0.eq(other)
    }
}

impl From<AdnlNodeIdShort> for [u8; 32] {
    fn from(id: AdnlNodeIdShort) -> Self {
        id.0
    }
}

impl From<AdnlNodeIdShort> for ton::int256 {
    fn from(id: AdnlNodeIdShort) -> Self {
        ton::int256(id.0)
    }
}

impl AsRef<[u8; 32]> for AdnlNodeIdShort {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

pub trait ComputeNodeIds {
    fn compute_node_ids(&self) -> Result<(AdnlNodeIdFull, AdnlNodeIdShort)>;
}

impl ComputeNodeIds for ed25519_consensus::SigningKey {
    fn compute_node_ids(&self) -> Result<(AdnlNodeIdFull, AdnlNodeIdShort)> {
        let public_key = ed25519_consensus::VerificationKey::from(self);
        let full_id = AdnlNodeIdFull::new(public_key);
        let short_id = full_id.compute_short_id()?;
        Ok((full_id, short_id))
    }
}

impl ComputeNodeIds for ed25519_consensus::VerificationKey {
    fn compute_node_ids(&self) -> Result<(AdnlNodeIdFull, AdnlNodeIdShort)> {
        let full_id = AdnlNodeIdFull::new(*self);
        let short_id = full_id.compute_short_id()?;
        Ok((full_id, short_id))
    }
}

#[derive(thiserror::Error, Debug)]
enum AdnlNodeIdError {
    #[error("Unsupported public key")]
    UnsupportedPublicKey,
}

pub struct StoredAdnlNodeKey {
    short_id: AdnlNodeIdShort,
    full_id: AdnlNodeIdFull,
    private_key: ed25519_consensus::SigningKey,
}

impl StoredAdnlNodeKey {
    pub fn from_id_and_private_key(
        short_id: AdnlNodeIdShort,
        full_id: AdnlNodeIdFull,
        private_key: &ed25519_consensus::SigningKey,
    ) -> Self {
        Self {
            short_id,
            full_id,
            private_key: private_key.clone(),
        }
    }

    pub fn id(&self) -> &AdnlNodeIdShort {
        &self.short_id
    }

    pub fn full_id(&self) -> &AdnlNodeIdFull {
        &self.full_id
    }

    pub fn private_key(&self) -> &ed25519_consensus::SigningKey {
        &self.private_key
    }

    pub fn sign(&self, data: &[u8]) -> ed25519_consensus::Signature {
        self.private_key.sign(data)
    }

    pub fn sign_boxed<T, F, R>(&self, data: T, inserter: F) -> Result<R>
    where
        T: IntoBoxed,
        F: FnOnce(T::Boxed, ton::bytes) -> R,
    {
        let data = data.into_boxed();
        let mut buffer = serialize(&data)?;
        let signature = self.sign(&buffer);
        buffer.truncate(0);
        let signature_bytes: [u8; 64] = signature.try_into()?;

        buffer.extend_from_slice(signature_bytes.as_ref());
        Ok(inserter(data, ton::bytes(buffer)))
    }
}
