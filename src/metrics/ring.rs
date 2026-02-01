use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::RwLock;

use super::types::RequestRecord;

#[derive(Clone)]
pub struct RequestRingBuffer {
    capacity: usize,
    records: Arc<RwLock<VecDeque<RequestRecord>>>,
}

impl RequestRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: Arc::new(RwLock::new(VecDeque::with_capacity(capacity))),
        }
    }

    pub fn push(&self, record: RequestRecord) {
        let mut records = self.records.write();
        if records.len() == self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    pub fn snapshot(&self) -> Vec<RequestRecord> {
        let records = self.records.read();
        records.iter().cloned().collect()
    }
}
