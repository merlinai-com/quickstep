use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Mutex,
};

use crate::map_table::PageId;

const RECORD_TYPE_PUT: u8 = 0;
const RECORD_TYPE_TOMBSTONE: u8 = 1;

#[derive(Clone, Debug)]
pub struct WalRecord {
    pub page_id: u64,
    pub disk_addr: u64,
    pub key: Vec<u8>,
    pub op: WalOp,
}

#[derive(Clone, Debug)]
pub enum WalOp {
    Put { value: Vec<u8> },
    Tombstone,
}

struct LeafWalStats {
    page_id: u64,
    count: usize,
    bytes: usize,
}

struct WalState {
    file: File,
    records: Vec<WalRecord>,
    leaf_counts: HashMap<u64, LeafWalStats>,
    total_records: usize,
    total_bytes: usize,
}

pub struct WalManager {
    state: Mutex<WalState>,
}

impl WalManager {
    pub fn open(path: &Path) -> io::Result<WalManager> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;

        let (records, truncated_len) = read_records(&mut file)?;
        if let Some(valid_len) = truncated_len {
            file.set_len(valid_len)?;
        }
        file.seek(SeekFrom::End(0))?;

        let mut leaf_counts = HashMap::new();
        let mut total_bytes = 0usize;
        for record in records.iter() {
            let entry = leaf_counts.entry(record.disk_addr).or_insert(LeafWalStats {
                page_id: record.page_id,
                count: 0,
                bytes: 0,
            });
            entry.page_id = record.page_id;
            entry.count += 1;
            let size = record_size(record);
            entry.bytes = entry.bytes.saturating_add(size);
            total_bytes = total_bytes.saturating_add(size);
        }
        let total_records = records.len();

        Ok(WalManager {
            state: Mutex::new(WalState {
                file,
                records,
                leaf_counts,
                total_records,
                total_bytes,
            }),
        })
    }

    pub fn records(&self) -> Vec<WalRecord> {
        let state = self.state.lock().expect("wal mutex poisoned");
        state.records.clone()
    }

    pub fn append_tombstone(&self, page_id: PageId, disk_addr: u64, key: &[u8]) -> io::Result<()> {
        self.append_record(WalRecord {
            page_id: page_id.as_u64(),
            disk_addr,
            key: key.to_vec(),
            op: WalOp::Tombstone,
        })
    }

    pub fn append_put(
        &self,
        page_id: PageId,
        disk_addr: u64,
        key: &[u8],
        value: &[u8],
    ) -> io::Result<()> {
        self.append_record(WalRecord {
            page_id: page_id.as_u64(),
            disk_addr,
            key: key.to_vec(),
            op: WalOp::Put {
                value: value.to_vec(),
            },
        })
    }

    fn append_record(&self, record: WalRecord) -> io::Result<()> {
        let mut state = self.state.lock().expect("wal mutex poisoned");
        let record_len = record_size(&record);
        state.records.push(record.clone());
        state.total_records += 1;
        state.total_bytes = state
            .total_bytes
            .checked_add(record_len)
            .expect("wal byte counter overflow");
        let entry = state
            .leaf_counts
            .entry(record.disk_addr)
            .or_insert(LeafWalStats {
                page_id: record.page_id,
                count: 0,
                bytes: 0,
            });
        entry.page_id = record.page_id;
        entry.count += 1;
        entry.bytes = entry.bytes.saturating_add(record_len);
        write_record(&mut state.file, &record)?;
        state.file.sync_data()?;
        Ok(())
    }

    pub fn checkpoint_leaf(&self, disk_addr: u64) -> io::Result<()> {
        let mut state = self.state.lock().expect("wal mutex poisoned");
        if state
            .records
            .iter()
            .all(|record| record.disk_addr != disk_addr)
        {
            return Ok(());
        }
        let removed = state.leaf_counts.remove(&disk_addr);
        state.records.retain(|record| record.disk_addr != disk_addr);
        if let Some(stats) = removed {
            state.total_records = state.total_records.saturating_sub(stats.count);
            state.total_bytes = state.total_bytes.saturating_sub(stats.bytes);
        }
        let snapshot = state.records.clone();
        rewrite_records(&mut state.file, &snapshot)?;
        Ok(())
    }

    pub fn clear(&self) -> io::Result<()> {
        let mut state = self.state.lock().expect("wal mutex poisoned");
        state.records.clear();
        state.leaf_counts.clear();
        state.total_records = 0;
        state.total_bytes = 0;
        state.file.set_len(0)?;
        state.file.sync_data()?;
        state.file.seek(SeekFrom::Start(0))?;
        Ok(())
    }

    pub fn should_checkpoint(&self, disk_addr: u64, threshold: usize) -> bool {
        let state = self.state.lock().expect("wal mutex poisoned");
        state
            .leaf_counts
            .get(&disk_addr)
            .map(|stats| stats.count >= threshold)
            .unwrap_or(false)
    }

    pub fn total_records(&self) -> usize {
        let state = self.state.lock().expect("wal mutex poisoned");
        state.total_records
    }

    pub fn total_bytes(&self) -> usize {
        let state = self.state.lock().expect("wal mutex poisoned");
        state.total_bytes
    }

    pub fn leaf_stats(&self, disk_addr: u64) -> Option<(u64, usize, usize)> {
        let state = self.state.lock().expect("wal mutex poisoned");
        state
            .leaf_counts
            .get(&disk_addr)
            .map(|stats| (stats.page_id, stats.count, stats.bytes))
    }

    pub fn global_checkpoint_candidate(
        &self,
        total_record_threshold: usize,
        total_byte_threshold: usize,
    ) -> Option<(u64, PageId)> {
        let state = self.state.lock().expect("wal mutex poisoned");
        if state.total_records < total_record_threshold && state.total_bytes < total_byte_threshold
        {
            return None;
        }
        state
            .leaf_counts
            .iter()
            .max_by_key(|(_, stats)| stats.bytes)
            .map(|(disk, stats)| (*disk, PageId(stats.page_id)))
    }
}

fn rewrite_records(file: &mut File, records: &[WalRecord]) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    for record in records {
        write_record(file, record)?;
    }
    file.sync_data()?;
    Ok(())
}

fn write_record(file: &mut File, record: &WalRecord) -> io::Result<()> {
    match &record.op {
        WalOp::Put { value } => {
            file.write_all(&[RECORD_TYPE_PUT])?;
            file.write_all(&record.page_id.to_le_bytes())?;
            file.write_all(&record.disk_addr.to_le_bytes())?;
            let key_len = record.key.len() as u32;
            let val_len = value.len() as u32;
            file.write_all(&key_len.to_le_bytes())?;
            file.write_all(&val_len.to_le_bytes())?;
            file.write_all(&record.key)?;
            file.write_all(value)?;
        }
        WalOp::Tombstone => {
            file.write_all(&[RECORD_TYPE_TOMBSTONE])?;
            file.write_all(&record.page_id.to_le_bytes())?;
            file.write_all(&record.disk_addr.to_le_bytes())?;
            let key_len = record.key.len() as u32;
            file.write_all(&key_len.to_le_bytes())?;
            file.write_all(&record.key)?;
        }
    }
    Ok(())
}

fn read_records(file: &mut File) -> io::Result<(Vec<WalRecord>, Option<u64>)> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut idx = 0usize;
    let mut records = Vec::new();
    while idx < bytes.len() {
        let record_start = idx;
        if bytes.len() - idx < 1 + 8 + 8 + 4 {
            break;
        }
        let record_type = bytes[idx];
        idx += 1;
        let page_id = u64::from_le_bytes(bytes[idx..idx + 8].try_into().unwrap());
        idx += 8;
        let disk_addr = u64::from_le_bytes(bytes[idx..idx + 8].try_into().unwrap());
        idx += 8;
        match record_type {
            RECORD_TYPE_TOMBSTONE => {
                if bytes.len() - idx < 4 {
                    idx = record_start;
                    break;
                }
                let key_len = u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                idx += 4;
                if bytes.len() - idx < key_len {
                    idx = record_start;
                    break;
                }
                let key = bytes[idx..idx + key_len].to_vec();
                idx += key_len;
                records.push(WalRecord {
                    page_id,
                    disk_addr,
                    key,
                    op: WalOp::Tombstone,
                });
            }
            RECORD_TYPE_PUT => {
                if bytes.len() - idx < 8 {
                    idx = record_start;
                    break;
                }
                let key_len = u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                idx += 4;
                let val_len = u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                idx += 4;
                if bytes.len() - idx < key_len + val_len {
                    idx = record_start;
                    break;
                }
                let key = bytes[idx..idx + key_len].to_vec();
                idx += key_len;
                let value = bytes[idx..idx + val_len].to_vec();
                idx += val_len;
                records.push(WalRecord {
                    page_id,
                    disk_addr,
                    key,
                    op: WalOp::Put { value },
                });
            }
            _ => {
                idx = record_start;
                break;
            }
        }
    }
    let truncated = if idx < bytes.len() {
        Some(idx as u64)
    } else {
        None
    };
    Ok((records, truncated))
}

fn record_size(record: &WalRecord) -> usize {
    match &record.op {
        WalOp::Put { value } => 1 + 8 + 8 + 4 + 4 + record.key.len() + value.len(),
        WalOp::Tombstone => 1 + 8 + 8 + 4 + record.key.len(),
    }
}
