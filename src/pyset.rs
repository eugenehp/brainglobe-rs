//! `list(set(ints))` in the order CPython iterates a set.
//!
//! `get_structures_at_hierarchy_level` returns `list(set(...))` of structure
//! ids, so its order is the slot order of CPython's set hash table. This is
//! that table (`Objects/setobject.c`, CPython 3.12 – 3.14): open addressing
//! with linear probes, then perturbation, resizing to 4x `used` when 60%
//! full. For ints `hash(n) == n` (modulo 2**61 - 1, with -1 mapped to -2).

const MINSIZE: usize = 8;
const LINEAR_PROBES: usize = 9;
const PERTURB_SHIFT: u32 = 5;

/// `hash(n)` for a Python int.
pub fn py_hash_int(n: i64) -> i64 {
    const MODULUS: i64 = (1 << 61) - 1;
    let r = if n >= 0 {
        n % MODULUS
    } else {
        -((-(n as i128) % MODULUS as i128) as i64)
    };
    if r == -1 { -2 } else { r }
}

#[derive(Clone, Copy)]
struct Entry {
    key: i64,
    hash: i64,
}

/// A CPython set of ints.
pub struct PySet {
    table: Vec<Option<Entry>>,
    fill: usize,
    used: usize,
}

impl Default for PySet {
    fn default() -> Self {
        PySet::new()
    }
}

impl PySet {
    pub fn new() -> PySet {
        PySet {
            table: vec![None; MINSIZE],
            fill: 0,
            used: 0,
        }
    }

    fn mask(&self) -> usize {
        self.table.len() - 1
    }

    /// `set.add(key)`.
    pub fn add(&mut self, key: i64) {
        let hash = py_hash_int(key);
        let mask = self.mask();
        let mut perturb = hash as u64 as usize;
        let mut i = (hash as u64 as usize) & mask;
        loop {
            match self.table[i] {
                None => break,
                Some(e) if e.hash == hash && e.key == key => return,
                _ => {}
            }
            if i + LINEAR_PROBES <= mask {
                let mut found = None;
                for j in 1..=LINEAR_PROBES {
                    match self.table[i + j] {
                        None => {
                            found = Some(i + j);
                            break;
                        }
                        Some(e) if e.hash == hash && e.key == key => return,
                        _ => {}
                    }
                }
                if let Some(slot) = found {
                    i = slot;
                    break;
                }
            }
            perturb >>= PERTURB_SHIFT;
            i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb)) & mask;
        }
        self.table[i] = Some(Entry { key, hash });
        self.fill += 1;
        self.used += 1;
        if self.fill * 5 < mask * 3 {
            return;
        }
        let minused = if self.used > 50000 {
            self.used * 2
        } else {
            self.used * 4
        };
        self.resize(minused);
    }

    fn resize(&mut self, minused: usize) {
        let mut newsize = MINSIZE;
        while newsize <= minused {
            newsize <<= 1;
        }
        let old = std::mem::replace(&mut self.table, vec![None; newsize]);
        self.fill = self.used;
        let mask = newsize - 1;
        for e in old.into_iter().flatten() {
            // set_insert_clean
            let mut perturb = e.hash as u64 as usize;
            let mut i = (e.hash as u64 as usize) & mask;
            'probe: loop {
                if self.table[i].is_none() {
                    break;
                }
                if i + LINEAR_PROBES <= mask {
                    for j in 1..=LINEAR_PROBES {
                        if self.table[i + j].is_none() {
                            i += j;
                            break 'probe;
                        }
                    }
                }
                perturb >>= PERTURB_SHIFT;
                i = (i.wrapping_mul(5).wrapping_add(1).wrapping_add(perturb)) & mask;
            }
            self.table[i] = Some(e);
        }
    }

    /// `list(s)`: the keys in slot order.
    pub fn to_list(&self) -> Vec<i64> {
        self.table.iter().flatten().map(|e| e.key).collect()
    }
}

/// `list(set(items))`.
pub fn py_set_list(items: impl IntoIterator<Item = i64>) -> Vec<i64> {
    let mut s = PySet::new();
    for k in items {
        s.add(k);
    }
    s.to_list()
}
