use arena::Arena;
use keccak_hash::{keccak, H256, KECCAK_NULL_RLP};
use node::Node;
use std::collections::HashMap;
use std::mem;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Index {
    Hash(usize),
    Memory(usize),
}

/// A Merkle Storage
///
/// Nodes are either stored in a simple Vec memory
/// or pushed into a *database* with key = sha3(rlp(value))
#[derive(Debug)]
pub struct Db {
    hash: HashMap<usize, Node>,
    memory: Vec<Node>,
    available_hash_slots: Vec<usize>,
    empty: usize,
    root: Index,
}

impl Db {
    pub fn new(arena: &mut Arena) -> Self {
        let idx = arena.push(KECCAK_NULL_RLP.as_ref());
        let mut hash = HashMap::new();
        hash.insert(idx, Node::Empty);
        Db {
            hash,
            memory: Vec::new(),
            root: Index::Hash(idx),
            available_hash_slots: Vec::new(),
            empty: idx,
        }
    }

    pub fn root_index(&self) -> Index {
        self.root
    }

    pub fn root<'a>(&self, arena: &'a Arena) -> Option<&'a [u8]> {
        match self.root_index() {
            Index::Memory(_) => None,
            Index::Hash(idx) => Some(&arena[idx]),
        }
    }

    pub fn get<'a>(&'a self, key: &Index) -> Option<&'a Node> {
        match key {
            Index::Hash(ref key) => self.hash.get(key),
            Index::Memory(ref key) => self.memory.get(*key),
        }
    }

    /// Get a mutable reference to node at key
    ///
    /// The reference index is, if needed, moved out of hash and into memory
    pub fn get_mut<'a>(&'a mut self, key: &mut Index) -> Option<&'a mut Node> {
        match *key {
            Index::Hash(hash) => {
                let node = self.hash.remove(&hash)?;
                let len = self.memory.len();
                if *key == self.root {
                    self.root = Index::Memory(len);
                }
                debug!("hash {} moved to memory {}", hash, len);
                self.available_hash_slots.push(hash);
                *key = Index::Memory(len);
                self.memory.push(node);
                self.memory.get_mut(len)
            }
            Index::Memory(ref key) => self.memory.get_mut(*key),
        }
    }

    pub fn insert_node(&mut self, key: Index, value: Node) -> Option<Node> {
        debug!("inserting node {:?}", key);
        match key {
            Index::Hash(key) => self.hash.insert(key, value),
            Index::Memory(key) => self.memory.get_mut(key).map(|v| mem::replace(v, value)),
        }
    }

    pub fn push_node(&mut self, node: Node) -> Index {
        let index = Index::Memory(self.memory.len());
        debug!("pushing node {:?}: {:?}", index, node);
        self.memory.push(node);
        index
    }

    pub fn remove(&mut self, key: &Index) -> Option<Node> {
        debug!("removing node {:?}", key);
        match key {
            Index::Hash(key) => self.hash.insert(*key, Node::Empty),
            Index::Memory(key) => self
                .memory
                .get_mut(*key)
                .map(|v| mem::replace(v, Node::Empty)),
        }
    }

    /// Commit all the in memory nodes into hash db
    pub fn commit(&mut self, arena: &mut Arena) -> Vec<(usize, usize)> {
        let mut hashed = Vec::with_capacity(self.memory.len());
        if let Index::Hash(_) = self.root {
            return hashed;
        }
        let mut index = self.root;
        self.commit_node(&mut index, arena, &mut hashed);
        self.memory.clear();
        self.root = index;
        hashed
    }

    fn commit_node(
        &mut self,
        index: &mut Index,
        arena: &mut Arena,
        hashed: &mut Vec<(usize, usize)>,
    ) {
        let mut node = match *index {
            Index::Hash(_) => return,
            Index::Memory(i) => mem::replace(&mut self.memory[i], Node::Empty),
        };

        let encoded_idx = match node {
            Node::Leaf(ref leaf) => leaf.encoded(arena),
            Node::Branch(ref mut branch) => {
                for k in &mut branch.keys {
                    if let Some(ref mut k) = k {
                        self.commit_node(k, arena, hashed);
                    }
                }
                branch.encoded(arena)
            }
            Node::Extension(ref mut ext) => {
                self.commit_node(&mut ext.key, arena, hashed);
                ext.encoded_or_empty(arena, self.empty)
            }
            Node::Empty => self.empty,
        };

        let hash = {
            let data = &arena[encoded_idx];
            if *index == self.root || data.len() >= H256::len() {
                Some(keccak(data))
            } else {
                None
            }
        };

        if let Some(hash) = hash {
            let hash_idx = if let Some(hash_idx) = self.available_hash_slots.pop() {
                arena.insert(hash_idx, hash.as_ref());
                hash_idx
            } else {
                arena.push(hash.as_ref())
            };
            self.hash.insert(hash_idx, node);
            hashed.push((hash_idx, encoded_idx));
            *index = Index::Hash(hash_idx);
        } else {
            // there is no need to save it in the database as we can directly decode it
            *index = Index::Hash(encoded_idx);
            self.hash.insert(encoded_idx, node);
        }
    }
}
