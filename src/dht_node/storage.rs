use std::convert::TryFrom;
use std::ops::Deref;

use anyhow::Result;
use smallvec::SmallVec;
use tl_proto::{BoxedConstructor, HashWrapper, TlWrite};

use super::DHT_KEY_NODES;
use crate::proto;
use crate::utils::*;

#[derive(Default)]
pub struct Storage {
    storage: FxDashMap<StorageKey, proto::dht::ValueOwned>,
}

impl Storage {
    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn total_size(&self) -> usize {
        self.storage.iter().map(|item| item.value.len()).sum()
    }

    pub fn get_ref(
        &self,
        key: &StorageKey,
    ) -> Option<impl Deref<Target = proto::dht::ValueOwned> + '_> {
        match self.storage.get(key) {
            Some(item) if item.ttl as u32 > now() => Some(item),
            _ => None,
        }
    }

    pub fn insert_signed_value(
        &self,
        key: StorageKey,
        mut value: proto::dht::Value<'_>,
    ) -> Result<bool> {
        use dashmap::mapref::entry::Entry;

        let full_id = AdnlNodeIdFull::try_from(value.key.id)?;

        let key_signature = std::mem::take(&mut value.key.signature);
        full_id.verify(&value.key, key_signature)?;
        value.key.signature = key_signature;

        let value_signature = std::mem::take(&mut value.signature);
        full_id.verify(&value, value_signature)?;
        value.signature = value_signature;

        Ok(match self.storage.entry(key) {
            Entry::Occupied(mut entry) if entry.get().ttl < value.ttl => {
                entry.insert(value.as_equivalent_owned());
                true
            }
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert(value.as_equivalent_owned());
                true
            }
        })
    }

    pub fn insert_overlay_nodes(&self, key: StorageKey, value: proto::dht::Value) -> Result<bool> {
        use dashmap::mapref::entry::Entry;

        if !value.signature.is_empty() || !value.key.signature.is_empty() {
            return Err(StorageError::InvalidSignatureValue.into());
        }

        let overlay_id = match value.key.id {
            everscale_crypto::tl::PublicKey::Overlay { .. } => {
                OverlayIdShort::from(tl_proto::hash(value.key.id))
            }
            _ => return Err(StorageError::InvalidKeyDescription.into()),
        };

        if make_dht_key(&overlay_id, DHT_KEY_NODES) != value.key.key {
            return Err(StorageError::InvalidDhtKey.into());
        }

        let mut new_nodes = deserialize_overlay_nodes(value.value)?;
        new_nodes.retain(|node| {
            if verify_node(&overlay_id, node).is_err() {
                tracing::warn!("Bad overlay node: {node:?}");
                false
            } else {
                true
            }
        });
        if new_nodes.is_empty() {
            return Err(StorageError::EmptyOverlayNodes.into());
        }

        match self.storage.entry(key) {
            Entry::Occupied(mut entry) => {
                let value = {
                    let old_nodes = match entry.get().ttl as u32 {
                        old_ttl if old_ttl < now() => None,
                        old_ttl if old_ttl > value.ttl as u32 => return Ok(false),
                        _ => Some(deserialize_overlay_nodes(&entry.get().value)?),
                    };
                    make_overlay_nodes_value(value, new_nodes, old_nodes)
                };
                entry.insert(value);
            }
            Entry::Vacant(entry) => {
                entry.insert(make_overlay_nodes_value(value, new_nodes, None));
            }
        }

        Ok(true)
    }
}

fn make_overlay_nodes_value<'a, 'b, const N: usize>(
    value: proto::dht::Value<'a>,
    new_nodes: SmallVec<[proto::overlay::Node<'a>; N]>,
    old_nodes: Option<SmallVec<[proto::overlay::Node<'b>; N]>>,
) -> proto::dht::ValueOwned {
    use std::collections::hash_map::Entry;

    let mut result = match old_nodes {
        Some(nodes) => nodes
            .into_iter()
            .map(|item| (HashWrapper(item.id), item))
            .collect::<FxHashMap<_, _>>(),
        None => Default::default(),
    };

    for node in new_nodes {
        match result.entry(HashWrapper(node.id)) {
            Entry::Occupied(mut entry) => {
                if entry.get().version < node.version {
                    entry.insert(node);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(node);
            }
        }
    }

    let capacity = result
        .values()
        .map(|item| item.max_size_hint())
        .sum::<usize>();

    let mut stored_value = Vec::with_capacity(4 + 4 + capacity);
    stored_value.extend_from_slice(&proto::overlay::Nodes::TL_ID.to_le_bytes());
    stored_value.extend_from_slice(&(result.len() as u32).to_le_bytes());
    for node in result.into_values() {
        node.write_to(&mut stored_value);
    }

    proto::dht::ValueOwned {
        key: value.key.as_equivalent_owned(),
        value: stored_value,
        ttl: value.ttl,
        signature: value.signature.to_vec(),
    }
}

fn deserialize_overlay_nodes(
    data: &[u8],
) -> tl_proto::TlResult<SmallVec<[proto::overlay::Node; 5]>> {
    let tl_proto::BoxedReader(proto::overlay::Nodes { nodes }) = tl_proto::deserialize(data)?;
    Ok(nodes)
}

pub type StorageKey = [u8; 32];

#[derive(thiserror::Error, Debug)]
enum StorageError {
    #[error("Invalid signature value")]
    InvalidSignatureValue,
    #[error("Invalid key description for OverlayNodes")]
    InvalidKeyDescription,
    #[error("Invalid DHT key")]
    InvalidDhtKey,
    #[error("Empty overlay nodes list")]
    EmptyOverlayNodes,
}
