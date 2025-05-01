use std::sync::Arc;
use yrs::{Doc, Map, MapRef, StateVector, Transact};
use serde::{Serialize, Deserialize};
use super::StorageLike;
use yrs::{ReadTxn, WriteTxn};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use ts_rs::TS;

#[derive(TS)]
#[ts(export, rename = "Y.Doc")]
pub struct YDoc(());

#[derive(TS)]
#[ts(export, rename = "Y.Map<any>")]
pub struct YMap(());

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[ts(rename = "Uint8Array")]
pub struct YrsStateVector(Vec<u8>);

impl From<StateVector> for YrsStateVector {
    fn from(sv: StateVector) -> Self {
        Self(sv.encode_v1())
    }
}

impl From<YrsStateVector> for StateVector {
    fn from(sv: YrsStateVector) -> Self {
        StateVector::decode_v1(&sv.0).unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[ts(rename = "Uint8Array")]
pub struct YrsOperation(Vec<u8>);

#[derive(Debug, Clone, TS)]
#[ts(export)]
pub struct YrsStorage {
    #[ts(type = "Y.Doc")]
    doc: Arc<Doc>,
    #[ts(type = "Y.Map<any>")]
    map: MapRef,
}

impl Default for YrsStorage {
    fn default() -> Self {
        let doc = Doc::new();
        let map = doc.get_or_insert_map("root");
        Self {
            doc: Arc::new(doc),
            map,
        }
    }
}

impl StorageLike for YrsStorage {
    type Version = YrsStateVector;
    type Operation = Vec<u8>;  // Binary update format that Yjs clients can send

    fn version(&self) -> Self::Version {
        YrsStateVector(self.doc.transact().state_vector().encode_v1())
    }

    fn apply_operation(&mut self, operation: Self::Operation) -> Result<Self::Version, super::StorageError> {
        let update = yrs::Update::decode_v1(&operation)
            .map_err(|e| super::StorageError::ApplyError(e.to_string()))?;
        self.doc.transact_mut()
            .apply_update(update)
            .map_err(|e| super::StorageError::ApplyError(e.to_string()))?;
        Ok(self.version())
    }

    fn merge(&mut self, other: &Self) -> Result<(), super::StorageError> {
        let other_state = other.doc.transact().state_vector();
        let update = self.doc.transact_mut().encode_update_v1();
        self.apply_operation(update)?;
        Ok(())
    }
}

impl Serialize for YrsStorage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let txn = self.doc.transact_mut();
        let update = txn.encode_update_v1();
        serializer.serialize_bytes(&update)
    }
}

impl<'de> Deserialize<'de> for YrsStorage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes: Vec<u8> = Deserialize::deserialize(deserializer)?;
        let doc = Doc::new();
        let update = yrs::Update::decode_v1(&bytes)
            .map_err(serde::de::Error::custom)?;
        doc.transact_mut()
            .apply_update(update)
            .map_err(serde::de::Error::custom)?;
        let map = doc.get_or_insert_map("root");
        Ok(Self {
            doc: Arc::new(doc),
            map,
        })
    }
} 