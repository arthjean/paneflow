use std::collections::{BTreeMap, BTreeSet};

use crate::BrowserError;

#[derive(Default)]
pub struct FrameLedger {
    active: Option<(u64, u64)>,
    outstanding: BTreeMap<(u64, u64, u8), u64>,
    last_sequence: u64,
}

impl FrameLedger {
    pub fn has_outstanding(&self) -> bool {
        !self.outstanding.is_empty()
    }

    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }

    pub fn active(&self) -> Option<(u64, u64)> {
        self.active
    }

    pub fn owns(&self, document: u64, generation: u64, buffer: u8, sequence: u64) -> bool {
        self.outstanding.get(&(document, generation, buffer)) == Some(&sequence)
    }

    pub fn forget_pool(&mut self, document: u64, pool: u64) {
        self.outstanding.retain(|(key_document, key_pool, _), _| {
            !(*key_document == document && *key_pool == pool)
        });
    }

    pub fn retired_pool(&self, document: u64) -> Option<u64> {
        let active = self.active.filter(|(active, _)| *active == document)?.1;
        self.outstanding
            .keys()
            .filter(|(key_document, pool, _)| *key_document == document && *pool != active)
            .map(|(_, pool, _)| *pool)
            .next()
    }

    pub fn invalidate(&mut self) {
        self.active = None;
    }

    pub fn resize(&mut self, document: u64, generation: u64) -> Result<(), BrowserError> {
        if generation == 0
            || self
                .active
                .is_some_and(|(current, pool)| current == document && generation <= pool)
        {
            return Err(BrowserError::StaleGeneration);
        }
        let pools: BTreeSet<_> = self
            .outstanding
            .keys()
            .map(|(document, pool, _)| (*document, *pool))
            .collect();
        if pools.len() >= 2 {
            return Err(BrowserError::Busy);
        }
        self.active = Some((document, generation));
        self.last_sequence = 0;
        Ok(())
    }

    pub fn receive(
        &mut self,
        document: u64,
        generation: u64,
        buffer: u8,
        sequence: u64,
    ) -> Result<(), BrowserError> {
        if self.active != Some((document, generation)) {
            return Err(BrowserError::StaleGeneration);
        }
        let key = (document, generation, buffer);
        if buffer >= 3 || sequence <= self.last_sequence || self.outstanding.contains_key(&key) {
            return Err(BrowserError::InvalidFrame);
        }
        self.outstanding.insert(key, sequence);
        self.last_sequence = sequence;
        Ok(())
    }

    pub fn release(
        &mut self,
        document: u64,
        generation: u64,
        buffer: u8,
        sequence: u64,
    ) -> Result<(), BrowserError> {
        let key = (document, generation, buffer);
        if self.outstanding.get(&key) != Some(&sequence) {
            return Err(BrowserError::InvalidFrame);
        }
        self.outstanding.remove(&key);
        Ok(())
    }
}
