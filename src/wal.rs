use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Mutex,
};
use std::convert::TryInto;

use crate::map_table::PageId;

const RECORD_TYPE_PUT: u8 = 0;
const RECORD_TYPE_TOMBSTONE: u8 = 1;
const RECORD_TYPE_TXN_BEGIN: u8 = 2;
const RECORD_TYPE_TXN_COMMIT: u8 = 3;
const RECORD_TYPE_TXN_ABORT: u8 = 4;
pub const TXN_META_PAGE_ID: u64 = u64::MAX;
const GROUP_MARKER: u8 = 0xAA;
const GROUP_HEADER_LEN: usize = 1 + 8 + 4;
const MANIFEST_MAGIC: [u8; 4] = *b"WALM";
const MANIFEST_VERSION: u32 = 1;
const MANIFEST_LEN: u64 = 32;

#[derive(Clone, Debug)]
pub struct WalRecord {
    pub page_id: u64,
    pub key: Vec<u8>,
    pub lower_fence: Vec<u8>,
    pub upper_fence: Vec<u8>,
    pub kind: WalEntryKind,
    pub txn_id: u64,
    pub op: WalOp,
}

#[derive(Clone, Copy, Debug)]
pub enum WalEntryKind {
    Redo,
    Undo,
}

#[derive(Clone, Copy, Debug)]
pub enum WalTxnMarker {
    Begin,
    Commit,
    Abort,
}

#[derive(Clone, Debug)]
pub enum WalOp {
    Put { value: Vec<u8> },
    Tombstone,
    TxnMarker(WalTxnMarker),
}

impl WalEntryKind {
    fn as_byte(self) -> u8 {
        match self {
            WalEntryKind::Redo => 0,
            WalEntryKind::Undo => 1,
        }
    }

    fn from_byte(byte: u8) -> Self {
        match byte {
            1 => WalEntryKind::Undo,
            _ => WalEntryKind::Redo,
        }
    }
}

impl WalTxnMarker {
    fn to_record_type(self) -> u8 {
        match self {
            WalTxnMarker::Begin => RECORD_TYPE_TXN_BEGIN,
            WalTxnMarker::Commit => RECORD_TYPE_TXN_COMMIT,
            WalTxnMarker::Abort => RECORD_TYPE_TXN_ABORT,
        }
    }

    fn from_record_type(tag: u8) -> Option<Self> {
        match tag {
            RECORD_TYPE_TXN_BEGIN => Some(WalTxnMarker::Begin),
            RECORD_TYPE_TXN_COMMIT => Some(WalTxnMarker::Commit),
            RECORD_TYPE_TXN_ABORT => Some(WalTxnMarker::Abort),
            _ => None,
        }
    }
}

struct LeafWalStats {
    count: usize,
    bytes: usize,
}

#[derive(Clone, Copy)]
struct WalManifest {
    checkpoint_len: u64,
}

impl WalManifest {
    fn new() -> WalManifest {
        WalManifest {
            checkpoint_len: MANIFEST_LEN,
        }
    }
}

struct WalState {
    file: File,
    records: Vec<WalRecord>,
    leaf_counts: HashMap<u64, LeafWalStats>,
    total_records: usize,
    total_bytes: usize,
    manifest: WalManifest,
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

        let mut manifest = read_manifest(&mut file)?;
        let (records, page_bytes, valid_len) = read_records(&mut file)?;
        let file_len = file.metadata()?.len();
        if valid_len < file_len {
            file.set_len(valid_len)?;
        }
        if manifest.checkpoint_len > valid_len {
            manifest.checkpoint_len = valid_len;
            write_manifest(&mut file, manifest)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;

        let mut leaf_counts = HashMap::new();
        let total_bytes = valid_len as usize;
        for record in records.iter() {
            let entry = leaf_counts
                .entry(record.page_id)
                .or_insert(LeafWalStats { count: 0, bytes: 0 });
            entry.count += 1;
        }
        for (page_id, bytes) in page_bytes.into_iter() {
            leaf_counts
                .entry(page_id)
                .and_modify(|stats| stats.bytes = bytes)
                .or_insert(LeafWalStats { count: 0, bytes });
        }
        let total_records = records.len();

        Ok(WalManager {
            state: Mutex::new(WalState {
                file,
                records,
                leaf_counts,
                total_records,
                total_bytes,
                manifest,
            }),
        })
    }

    pub fn records(&self) -> Vec<WalRecord> {
        let state = self.state.lock().expect("wal mutex poisoned");
        state.records.clone()
    }

    pub fn records_grouped(&self) -> BTreeMap<u64, Vec<WalRecord>> {
        let state = self.state.lock().expect("wal mutex poisoned");
        let mut grouped: BTreeMap<u64, Vec<WalRecord>> = BTreeMap::new();
        for record in state.records.iter() {
            grouped
                .entry(record.page_id)
                .or_default()
                .push(record.clone());
        }
        grouped
    }

    pub fn append_tombstone(
        &self,
        page_id: PageId,
        key: &[u8],
        lower_fence: &[u8],
        upper_fence: &[u8],
        kind: WalEntryKind,
        txn_id: u64,
    ) -> io::Result<()> {
        self.append_record(WalRecord {
            page_id: page_id.as_u64(),
            key: key.to_vec(),
            lower_fence: lower_fence.to_vec(),
            upper_fence: upper_fence.to_vec(),
            kind,
            txn_id,
            op: WalOp::Tombstone,
        })
    }

    pub fn append_put(
        &self,
        page_id: PageId,
        key: &[u8],
        value: &[u8],
        lower_fence: &[u8],
        upper_fence: &[u8],
        kind: WalEntryKind,
        txn_id: u64,
    ) -> io::Result<()> {
        self.append_record(WalRecord {
            page_id: page_id.as_u64(),
            key: key.to_vec(),
            lower_fence: lower_fence.to_vec(),
            upper_fence: upper_fence.to_vec(),
            kind,
            txn_id,
            op: WalOp::Put {
                value: value.to_vec(),
            },
        })
    }

    pub fn append_txn_marker(
        &self,
        marker: WalTxnMarker,
        kind: WalEntryKind,
        txn_id: u64,
    ) -> io::Result<()> {
        self.append_record(WalRecord {
            page_id: TXN_META_PAGE_ID,
            key: Vec::new(),
            lower_fence: Vec::new(),
            upper_fence: Vec::new(),
            kind,
            txn_id,
            op: WalOp::TxnMarker(marker),
        })
    }

    fn append_record(&self, record: WalRecord) -> io::Result<()> {
        let mut state = self.state.lock().expect("wal mutex poisoned");
        state.file.seek(SeekFrom::End(0))?;
        state.records.push(record.clone());
        state.total_records += 1;
        state
            .leaf_counts
            .entry(record.page_id)
            .or_insert(LeafWalStats { count: 0, bytes: 0 })
            .count += 1;
        let bytes_written = write_group(
            &mut state.file,
            record.page_id,
            std::slice::from_ref(&record),
        )?;
        if let Some(entry) = state.leaf_counts.get_mut(&record.page_id) {
            entry.bytes = entry.bytes.saturating_add(bytes_written);
        }
        state.total_bytes = state
            .total_bytes
            .checked_add(bytes_written)
            .expect("wal byte counter overflow");
        state.file.sync_data()?;
        Ok(())
    }

    pub fn checkpoint_page(&self, page_id: PageId) -> io::Result<()> {
        let page_key = page_id.as_u64();
        let mut state = self.state.lock().expect("wal mutex poisoned");
        if state
            .records
            .iter()
            .all(|record| record.page_id != page_key)
        {
            return Ok(());
        }
        state.records.retain(|record| record.page_id != page_key);
        let snapshot = state.records.clone();
        let stats = rewrite_records(&mut state.file, &snapshot)?;
        state.leaf_counts = stats;
        state.total_records = state.records.len();
        state.total_bytes = state
            .leaf_counts
            .values()
            .fold(0usize, |acc, entry| acc.saturating_add(entry.bytes));
        state.manifest.checkpoint_len = MANIFEST_LEN + state.total_bytes as u64;
        let manifest = state.manifest;
        write_manifest(&mut state.file, manifest)?;
        state.file.sync_data()?;
        state.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub fn clear(&self) -> io::Result<()> {
        let mut state = self.state.lock().expect("wal mutex poisoned");
        state.records.clear();
        state.leaf_counts.clear();
        state.total_records = 0;
        state.total_bytes = 0;
        state.manifest = WalManifest::new();
        let manifest = state.manifest;
        state.file.set_len(MANIFEST_LEN)?;
        write_manifest(&mut state.file, manifest)?;
        state.file.sync_data()?;
        state.file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub fn should_checkpoint_page(&self, page_id: PageId, threshold: usize) -> bool {
        let state = self.state.lock().expect("wal mutex poisoned");
        state
            .leaf_counts
            .get(&page_id.as_u64())
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

    pub fn leaf_stats(&self, page_id: PageId) -> Option<(usize, usize)> {
        let state = self.state.lock().expect("wal mutex poisoned");
        state
            .leaf_counts
            .get(&page_id.as_u64())
            .map(|stats| (stats.count, stats.bytes))
    }

    pub fn global_checkpoint_candidate(
        &self,
        total_record_threshold: usize,
        total_byte_threshold: usize,
    ) -> Option<PageId> {
        let state = self.state.lock().expect("wal mutex poisoned");
        if state.total_records < total_record_threshold && state.total_bytes < total_byte_threshold
        {
            return None;
        }
        state
            .leaf_counts
            .iter()
            .filter(|(page, _)| **page != TXN_META_PAGE_ID)
            .max_by_key(|(_, stats)| stats.bytes)
            .map(|(page, _)| PageId(*page))
    }
}

fn rewrite_records(
    file: &mut File,
    records: &[WalRecord],
) -> io::Result<HashMap<u64, LeafWalStats>> {
    file.set_len(MANIFEST_LEN)?;
    file.seek(SeekFrom::Start(MANIFEST_LEN))?;
    let mut stats: HashMap<u64, LeafWalStats> = HashMap::new();
    let mut idx = 0usize;
    while idx < records.len() {
        let page_id = records[idx].page_id;
        let mut end = idx + 1;
        while end < records.len() && records[end].page_id == page_id {
            end += 1;
        }
        let bytes_written = write_group(file, page_id, &records[idx..end])?;
        stats
            .entry(page_id)
            .and_modify(|entry| {
                entry.count += end - idx;
                entry.bytes = entry.bytes.saturating_add(bytes_written);
            })
            .or_insert(LeafWalStats {
                count: end - idx,
                bytes: bytes_written,
            });
        idx = end;
    }
    file.sync_data()?;
    Ok(stats)
}

fn write_group(file: &mut File, page_id: u64, records: &[WalRecord]) -> io::Result<usize> {
    if records.is_empty() {
        return Ok(0);
    }
    file.write_all(&[GROUP_MARKER])?;
    file.write_all(&page_id.to_le_bytes())?;
    let count = u32::try_from(records.len()).expect("record group too large");
    file.write_all(&count.to_le_bytes())?;
    let mut payload = 0usize;
    for record in records {
        payload += write_record_payload(file, record)?;
    }
    Ok(GROUP_HEADER_LEN + payload)
}

fn write_record_payload(file: &mut File, record: &WalRecord) -> io::Result<usize> {
    match &record.op {
        WalOp::Put { value } => {
            file.write_all(&[RECORD_TYPE_PUT])?;
            file.write_all(&[record.kind.as_byte()])?;
            file.write_all(&record.txn_id.to_le_bytes())?;
            let header_bytes = 1 + 8;
            let key_len = record.key.len() as u32;
            let val_len = value.len() as u32;
            let lower_len = record.lower_fence.len() as u32;
            let upper_len = record.upper_fence.len() as u32;
            file.write_all(&key_len.to_le_bytes())?;
            file.write_all(&val_len.to_le_bytes())?;
            file.write_all(&lower_len.to_le_bytes())?;
            file.write_all(&upper_len.to_le_bytes())?;
            file.write_all(&record.key)?;
            file.write_all(value)?;
            file.write_all(&record.lower_fence)?;
            file.write_all(&record.upper_fence)?;
            Ok(header_bytes
                + 1
                + 4
                + 4
                + 4
                + 4
                + record.key.len()
                + value.len()
                + record.lower_fence.len()
                + record.upper_fence.len())
        }
        WalOp::Tombstone => {
            file.write_all(&[RECORD_TYPE_TOMBSTONE])?;
            file.write_all(&[record.kind.as_byte()])?;
            file.write_all(&record.txn_id.to_le_bytes())?;
            let header_bytes = 1 + 8;
            let key_len = record.key.len() as u32;
            let lower_len = record.lower_fence.len() as u32;
            let upper_len = record.upper_fence.len() as u32;
            file.write_all(&key_len.to_le_bytes())?;
            file.write_all(&lower_len.to_le_bytes())?;
            file.write_all(&upper_len.to_le_bytes())?;
            file.write_all(&record.key)?;
            file.write_all(&record.lower_fence)?;
            file.write_all(&record.upper_fence)?;
            Ok(header_bytes
                + 1
                + 4
                + 4
                + 4
                + record.key.len()
                + record.lower_fence.len()
                + record.upper_fence.len())
        }
        WalOp::TxnMarker(marker) => {
            file.write_all(&[marker.to_record_type()])?;
            file.write_all(&[record.kind.as_byte()])?;
            file.write_all(&record.txn_id.to_le_bytes())?;
            let header_bytes = 1 + 8;
            Ok(header_bytes + 1)
        }
    }
}

fn read_records(file: &mut File) -> io::Result<(Vec<WalRecord>, HashMap<u64, usize>, u64)> {
    file.seek(SeekFrom::Start(MANIFEST_LEN))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut idx = 0usize;
    let mut records = Vec::new();
    let mut page_bytes: HashMap<u64, usize> = HashMap::new();
    let mut valid_idx = 0usize;

    'outer: while bytes.len().saturating_sub(idx) >= GROUP_HEADER_LEN {
        if bytes[idx] != GROUP_MARKER {
            break;
        }
        idx += 1;
        let page_id = u64::from_le_bytes(bytes[idx..idx + 8].try_into().unwrap());
        idx += 8;
        let record_count = u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
        idx += 4;

        let mut payload_bytes = 0usize;
        let mut parsed = 0usize;
        while parsed < record_count {
            if idx >= bytes.len() {
                break 'outer;
            }
            let record_type = bytes[idx];
            idx += 1;
            if bytes.len() - idx < 1 + 8 {
                break 'outer;
            }
            let entry_kind = WalEntryKind::from_byte(bytes[idx]);
            idx += 1;
            let txn_id = u64::from_le_bytes(bytes[idx..idx + 8].try_into().unwrap());
            idx += 8;
            match record_type {
                RECORD_TYPE_TOMBSTONE => {
                    if bytes.len() - idx < 12 {
                        break 'outer;
                    }
                    let key_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    let lower_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    let upper_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    if bytes.len() - idx < key_len + lower_len + upper_len {
                        break 'outer;
                    }
                    let key = bytes[idx..idx + key_len].to_vec();
                    idx += key_len;
                    let lower = bytes[idx..idx + lower_len].to_vec();
                    idx += lower_len;
                    let upper = bytes[idx..idx + upper_len].to_vec();
                    idx += upper_len;
                    let record = WalRecord {
                        page_id,
                        key,
                        lower_fence: lower,
                        upper_fence: upper,
                        kind: entry_kind,
                        txn_id,
                        op: WalOp::Tombstone,
                    };
                    payload_bytes = payload_bytes.saturating_add(record_size(&record));
                    records.push(record);
                }
                RECORD_TYPE_PUT => {
                    if bytes.len() - idx < 16 {
                        break 'outer;
                    }
                    let key_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    let val_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    let lower_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    let upper_len =
                        u32::from_le_bytes(bytes[idx..idx + 4].try_into().unwrap()) as usize;
                    idx += 4;
                    if bytes.len() - idx < key_len + val_len + lower_len + upper_len {
                        break 'outer;
                    }
                    let key = bytes[idx..idx + key_len].to_vec();
                    idx += key_len;
                    let value = bytes[idx..idx + val_len].to_vec();
                    idx += val_len;
                    let lower = bytes[idx..idx + lower_len].to_vec();
                    idx += lower_len;
                    let upper = bytes[idx..idx + upper_len].to_vec();
                    idx += upper_len;
                    let record = WalRecord {
                        page_id,
                        key,
                        lower_fence: lower,
                        upper_fence: upper,
                        kind: entry_kind,
                        txn_id,
                        op: WalOp::Put { value },
                    };
                    payload_bytes = payload_bytes.saturating_add(record_size(&record));
                    records.push(record);
                }
                RECORD_TYPE_TXN_BEGIN | RECORD_TYPE_TXN_COMMIT | RECORD_TYPE_TXN_ABORT => {
                    let marker =
                        WalTxnMarker::from_record_type(record_type).expect("invalid txn marker");
                    let record = WalRecord {
                        page_id,
                        key: Vec::new(),
                        lower_fence: Vec::new(),
                        upper_fence: Vec::new(),
                        kind: entry_kind,
                        txn_id,
                        op: WalOp::TxnMarker(marker),
                    };
                    payload_bytes = payload_bytes.saturating_add(record_size(&record));
                    records.push(record);
                }
                _ => {
                    break 'outer;
                }
            }
            parsed += 1;
        }

        let group_bytes = GROUP_HEADER_LEN + payload_bytes;
        page_bytes
            .entry(page_id)
            .and_modify(|bytes| *bytes = bytes.saturating_add(group_bytes))
            .or_insert(group_bytes);
        valid_idx = idx;
    }

    let valid_len = MANIFEST_LEN + valid_idx as u64;
    Ok((records, page_bytes, valid_len))
}

fn record_size(record: &WalRecord) -> usize {
    match &record.op {
        WalOp::Put { value } => {
            1 + 8
                + 1
                + 4
                + 4
                + 4
                + 4
                + record.key.len()
                + value.len()
                + record.lower_fence.len()
                + record.upper_fence.len()
        }
        WalOp::Tombstone => {
            1 + 8
                + 1
                + 4
                + 4
                + 4
                + record.key.len()
                + record.lower_fence.len()
                + record.upper_fence.len()
        }
        WalOp::TxnMarker(_) => 1 + 8 + 1,
    }
}

fn read_manifest(file: &mut File) -> io::Result<WalManifest> {
    let mut manifest = WalManifest::new();
    let len = file.metadata()?.len();
    if len < MANIFEST_LEN {
        file.set_len(MANIFEST_LEN)?;
        write_manifest(file, manifest)?;
        file.sync_data()?;
        return Ok(manifest);
    }
    let mut header = [0u8; MANIFEST_LEN as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    if &header[0..4] != MANIFEST_MAGIC || u32::from_le_bytes(header[4..8].try_into().unwrap()) != MANIFEST_VERSION {
        write_manifest(file, manifest)?;
        file.sync_data()?;
        return Ok(manifest);
    }
    manifest.checkpoint_len =
        u64::from_le_bytes(header[8..16].try_into().unwrap()).max(MANIFEST_LEN);
    Ok(manifest)
}

fn write_manifest(file: &mut File, manifest: WalManifest) -> io::Result<()> {
    let mut buf = [0u8; MANIFEST_LEN as usize];
    buf[0..4].copy_from_slice(&MANIFEST_MAGIC);
    buf[4..8].copy_from_slice(&MANIFEST_VERSION.to_le_bytes());
    buf[8..16].copy_from_slice(&manifest.checkpoint_len.to_le_bytes());
    let current = file.seek(SeekFrom::Current(0))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&buf)?;
    file.seek(SeekFrom::Start(current))?;
    Ok(())
}
