#![expect(dead_code)]

//! A tri-key map to store output data

use std::sync::{Arc, Mutex, RwLock};

use dashmap::DashMap;
use smithay_client_toolkit::reexports::client::{Proxy, backend::ObjectId};

use super::output::OutputRepr;

// TODO: make this map generic

#[derive(Debug)]
pub enum OutputKey {
    OutputName(String),
    SurfaceId(ObjectId),
    OutputId(ObjectId),
}

/// Struct guarding access to output data
#[derive(Default, Debug, Clone)]
pub struct OutputMap {
    output_vec: Arc<RwLock<Vec<Arc<Mutex<OutputRepr>>>>>,
    output_name_map: Arc<DashMap<String, usize>>,
    surface_id_map: Arc<DashMap<ObjectId, usize>>,
    output_id_map: Arc<DashMap<ObjectId, usize>>,
}

impl OutputMap {
    /// Get vector index from key by map lookup
    fn get_idx(&self, key: OutputKey) -> Option<usize> {
        match key {
            OutputKey::OutputName(output_name) => {
                self.output_name_map.get(&output_name).map(|e| *e.value())
            }
            OutputKey::SurfaceId(surface_id) => {
                self.surface_id_map.get(&surface_id).map(|e| *e.value())
            }
            OutputKey::OutputId(output_id) => {
                self.output_id_map.get(&output_id).map(|e| *e.value())
            }
        }
    }

    /// Insert a new output
    pub fn insert(
        &mut self,
        output_name: String,
        surface_id: ObjectId,
        output_id: ObjectId,
        output: OutputRepr,
    ) {
        self.output_vec
            .write()
            .unwrap()
            .push(Arc::new(Mutex::new(output)));
        let idx = self.output_vec.read().unwrap().len() - 1;
        self.output_name_map.insert(output_name, idx);
        self.surface_id_map.insert(surface_id, idx);
        self.output_id_map.insert(output_id, idx);
    }

    /// Get the output based on the provided key and type
    pub fn get(&self, key: OutputKey) -> Option<Arc<Mutex<OutputRepr>>> {
        if let Some(idx) = self.get_idx(key) {
            self.output_vec.read().unwrap().get(idx).cloned()
        } else {
            None
        }
    }

    pub fn remove(&mut self, key: OutputKey) -> Arc<Mutex<OutputRepr>> {
        let idx = self.get_idx(key).expect("valid index returned");
        let removed = self.output_vec.write().unwrap().remove(idx);

        // Rebuild all index maps to avoid stale indices after vec removal.
        self.output_name_map.clear();
        self.surface_id_map.clear();
        self.output_id_map.clear();

        for (i, output) in self.output_vec.read().unwrap().iter().enumerate() {
            let output = output.lock().unwrap();
            self.output_name_map.insert(output.output_name.clone(), i);
            if let Some(surface) = output._surface.as_ref() {
                self.surface_id_map.insert(surface.id(), i);
            }
            self.output_id_map.insert(output._wl_repr.id(), i);
        }

        removed
    }

    /// Check if the relevant maps if the key exists
    pub fn contains_key(&self, key: OutputKey) -> bool {
        match key {
            OutputKey::OutputName(output_name) => self.output_name_map.contains_key(&output_name),
            OutputKey::SurfaceId(surface_id) => self.surface_id_map.contains_key(&surface_id),
            OutputKey::OutputId(output_id) => self.output_id_map.contains_key(&output_id),
        }
    }

    /// Get an iterator over the underlying collection
    pub fn iter(&self) -> OutputMapIter<'_> {
        OutputMapIter {
            map: self,
            index: 0,
        }
    }
}
pub struct OutputMapIter<'a> {
    map: &'a OutputMap,
    index: usize,
}

impl Iterator for OutputMapIter<'_> {
    type Item = Arc<Mutex<OutputRepr>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.map.output_vec.read().unwrap().len() {
            let result = Some(&self.map.output_vec.read().unwrap()[self.index]).cloned();
            self.index += 1;
            result
        } else {
            None
        }
    }
}
