//! Lineage: ancestry records for every genome and queries over descent.
//! Pure — an in-memory tree the server can persist to SQLite.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Where a genome came from. `Init` = random at cold start; `Mutant` = child of
/// a parent genome via ES mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BornFrom {
    Init,
    Mutant,
}

/// One genome's ancestry record. `id`s are stable within a lineage store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineageRecord {
    pub id: u64,
    pub generation: u32,
    pub parent_id: Option<u64>,
    pub born_from: BornFrom,
}

/// An in-memory lineage registry. `id` assignment is monotonic and explicit so
/// it round-trips to the server's `genomes` table.
#[derive(Clone, Debug, Default)]
pub struct Lineage {
    records: HashMap<u64, LineageRecord>,
    next_id: u64,
}

impl Lineage {
    pub fn new() -> Self {
        Lineage {
            records: HashMap::new(),
            next_id: 1,
        }
    }

    /// Insert a record with an explicit id (e.g. restored from the database).
    pub fn insert_with_id(
        &mut self,
        id: u64,
        generation: u32,
        parent_id: Option<u64>,
        born_from: BornFrom,
    ) {
        if id >= self.next_id {
            self.next_id = id + 1;
        }
        self.records.insert(
            id,
            LineageRecord {
                id,
                generation,
                parent_id,
                born_from,
            },
        );
    }

    /// Register a new genome; returns its id.
    pub fn register(
        &mut self,
        generation: u32,
        parent_id: Option<u64>,
        born_from: BornFrom,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.records.insert(
            id,
            LineageRecord {
                id,
                generation,
                parent_id,
                born_from,
            },
        );
        id
    }

    pub fn get(&self, id: u64) -> Option<&LineageRecord> {
        self.records.get(&id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Ancestor chain from `id` back to the cold-start root, most-recent first
    /// (includes `id` itself).
    pub fn ancestors(&self, id: u64) -> Vec<LineageRecord> {
        let mut out = Vec::new();
        let mut cur = self.records.get(&id);
        while let Some(rec) = cur {
            out.push(rec.clone());
            cur = rec.parent_id.and_then(|p| self.records.get(&p));
        }
        out
    }

    /// All records, sorted by id (stable iteration for persistence).
    pub fn all(&self) -> Vec<LineageRecord> {
        let mut v: Vec<_> = self.records.values().cloned().collect();
        v.sort_by_key(|r| r.id);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestry_chain_reaches_root() {
        let mut l = Lineage::new();
        let root = l.register(0, None, BornFrom::Init);
        let gen1 = l.register(1, Some(root), BornFrom::Mutant);
        let gen2 = l.register(2, Some(gen1), BornFrom::Mutant);

        let chain = l.ancestors(gen2);
        let ids: Vec<u64> = chain.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![gen2, gen1, root]);

        // Root has no parent.
        assert_eq!(l.get(root).unwrap().parent_id, None);
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn explicit_ids_round_trip() {
        let mut l = Lineage::new();
        l.insert_with_id(7, 3, Some(2), BornFrom::Mutant);
        assert_eq!(l.get(7).unwrap().generation, 3);
        // Next auto id skips past explicit ids.
        let next = l.register(4, Some(7), BornFrom::Mutant);
        assert_eq!(next, 8);
    }
}
