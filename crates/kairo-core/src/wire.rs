//! Serde helpers for the model's wire form (`kairo serve` and its page):
//! the names Kairo knows (AI services, domestic AIs) are `&'static str` in
//! memory, and come back from the wire as the same static names; maps with
//! keys JSON cannot hold travel as lists of pairs.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A name Kairo knows, as the model's fields hold it. The alias keeps serde's
/// derive from taking the field for a borrowed string.
pub type Static = &'static str;

/// The static name equal to `name`: an AI service's id or a domestic AI's
/// name. A name Kairo does not know (a newer server) is `"unknown"`.
pub fn known(name: &str) -> &'static str {
    crate::ai::SERVICES
        .iter()
        .map(|s| s.id)
        .chain(crate::ai::DOMESTIC.iter().map(|(n, _)| *n))
        .find(|n| *n == name)
        .unwrap_or("unknown")
}

/// `&'static str` fields.
pub mod name {
    use super::*;

    pub fn serialize<S: Serializer>(v: &&'static str, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<&'static str, D::Error> {
        Ok(known(&String::deserialize(d)?))
    }
}

/// `Option<&'static str>` fields.
pub mod opt_name {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<&'static str>, s: S) -> Result<S::Ok, S::Error> {
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<&'static str>, D::Error> {
        Ok(Option::<String>::deserialize(d)?.map(|n| known(&n)))
    }
}

/// Maps whose keys JSON cannot hold (tuples, enums with data, static
/// names): a list of `[key, value]` pairs.
pub mod pairs {
    use super::*;

    pub fn serialize<'a, S, K, V, M>(map: &'a M, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        &'a M: IntoIterator<Item = (&'a K, &'a V)>,
        K: Serialize + 'a,
        V: Serialize + 'a,
    {
        s.collect_seq(map)
    }

    pub fn deserialize<'de, D, K, V, M>(d: D) -> Result<M, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
        M: FromIterator<(K, V)>,
    {
        Ok(Vec::<(K, V)>::deserialize(d)?.into_iter().collect())
    }
}

/// A static name as a map key or tuple member: serializes as the string,
/// deserializes to the known static name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(pub &'static str);

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
        Ok(Name(known(&String::deserialize(d)?)))
    }
}

/// `BTreeMap<&'static str, V>` fields: an object keyed by the names.
pub mod name_map {
    use std::collections::BTreeMap;

    use super::*;

    pub fn serialize<S: Serializer, V: Serialize>(
        map: &BTreeMap<&'static str, V>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.collect_map(map.iter())
    }

    pub fn deserialize<'de, D: Deserializer<'de>, V: Deserialize<'de>>(
        d: D,
    ) -> Result<BTreeMap<&'static str, V>, D::Error> {
        Ok(BTreeMap::<String, V>::deserialize(d)?
            .into_iter()
            .map(|(k, v)| (known(&k), v))
            .collect())
    }
}
