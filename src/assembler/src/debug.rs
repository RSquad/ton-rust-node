/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use serde::{
    de::{Error, MapAccess, Visitor},
    ser::SerializeMap,
    Deserialize, Serialize,
};
use std::collections::BTreeMap;
use ton_block::{Cell, UInt256};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbgPos {
    pub filename: String,
    pub line: usize,
}

impl std::fmt::Display for DbgPos {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let filename = if self.filename.is_empty() { "<none>" } else { self.filename.as_str() };
        write!(f, "{}:{}", filename, self.line)
    }
}

#[derive(Clone, Debug, Default)]
pub struct DbgNode {
    pub offsets: Vec<(usize, DbgPos)>,
    pub children: Vec<DbgNode>,
}

impl DbgNode {
    pub fn from_ext(pos: DbgPos, dbgs: Vec<DbgNode>) -> Self {
        Self { offsets: vec![(0, pos)], children: dbgs }
    }
    pub fn from(pos: DbgPos) -> Self {
        Self::from_ext(pos, vec![])
    }
    pub fn inline_node(&mut self, offset: usize, dbg: DbgNode) {
        for (o, p) in dbg.offsets {
            self.offsets.push((o + offset, p));
        }
        for child in dbg.children {
            self.append_node(child);
        }
    }
    pub fn append_node(&mut self, dbg: DbgNode) {
        assert!(self.children.len() < 4);
        self.children.push(dbg)
    }
}

impl std::fmt::Display for DbgNode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        for entry in self.offsets.iter() {
            writeln!(f, "{}:{}", entry.0, entry.1)?
        }
        write!(f, "{} children", self.children.len())
    }
}

#[derive(Default, PartialEq, Eq)]
pub struct DbgInfo {
    map: BTreeMap<[u8; 32], BTreeMap<usize, DbgPos>>,
}

impl Serialize for DbgInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.map.len()))?;
        for (k, v) in &self.map {
            map.serialize_entry(&hex::encode(k), v)?
        }
        map.end()
    }
}

struct DbgInfoVisitor {
    marker: std::marker::PhantomData<fn() -> DbgInfo>,
}

impl DbgInfoVisitor {
    fn new() -> Self {
        Self { marker: std::marker::PhantomData }
    }
}

impl<'a> Visitor<'a> for DbgInfoVisitor {
    type Value = DbgInfo;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a debug info map")
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'a>,
    {
        let mut map = BTreeMap::<[u8; 32], BTreeMap<usize, DbgPos>>::new();
        while let Some((key, value)) = access.next_entry()? {
            let v = hex::decode::<String>(key).map_err(M::Error::custom)?;
            let arr: [u8; 32] = v.try_into().map_err(|ev: Vec<u8>| {
                M::Error::custom(format!("bytestring size must be 32 not {}", ev.len()))
            })?;
            map.insert(arr, value);
        }
        Ok(DbgInfo { map })
    }
}

impl<'a> Deserialize<'a> for DbgInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        deserializer.deserialize_map(DbgInfoVisitor::new())
    }
}

impl std::fmt::Debug for DbgInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.map.iter()).finish()
    }
}

impl DbgInfo {
    pub fn from(cell: Cell, node: DbgNode) -> Self {
        let mut info = DbgInfo { map: BTreeMap::new() };
        info.collect(cell, node);
        info
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn append(&mut self, other: &mut Self) {
        self.map.append(&mut other.map);
    }
    pub fn insert(&mut self, key: UInt256, tree: BTreeMap<usize, DbgPos>) {
        self.map.entry(key.inner()).or_insert(tree);
    }
    pub fn remove(&mut self, key: &UInt256) -> Option<BTreeMap<usize, DbgPos>> {
        self.map.remove(key.as_slice())
    }
    pub fn get(&self, key: &UInt256) -> Option<&BTreeMap<usize, DbgPos>> {
        self.map.get(key.as_slice())
    }
    pub fn first_entry(&self) -> Option<&BTreeMap<usize, DbgPos>> {
        self.map.iter().next().map(|k_v| k_v.1)
    }
    fn collect(&mut self, cell: Cell, dbg: DbgNode) {
        let mut stack = vec![(cell.clone(), dbg)];
        while let Some((cell, mut dbg)) = stack.pop() {
            let hash = cell.repr_hash().inner();
            let offsets_len = dbg.offsets.len();
            self.map.insert(hash, dbg.offsets.into_iter().collect());
            debug_assert_eq!(Some(offsets_len), self.map.get(&hash).map(|v| v.len()));
            for i in 0..cell.references_count() {
                if i >= dbg.children.len() {
                    continue;
                }
                let child_cell = cell.reference(i).unwrap();
                let child_hash = child_cell.repr_hash().inner();
                if !self.map.contains_key(&child_hash) {
                    let child_dbg = std::mem::take(&mut dbg.children[i]);
                    stack.push((child_cell, child_dbg));
                }
            }
        }
    }
}
