use std::{
    fs::{self, File, OpenOptions},
    mem::MaybeUninit,
    os::unix::fs::FileExt,
    path::Path,
};

use crate::{lock_manager::WriteGuardWrapper, types::NodeMeta};

pub struct IoEngine {
    file: File,
}

impl IoEngine {
    pub fn open(path: &Path) -> std::io::Result<IoEngine> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;

        Ok(IoEngine { file })
    }

    /// Get the page of the given address
    pub fn get_page(&self, page_addr: u64) -> DiskLeaf {
        // SAFETY: this immediately overwritten
        let mut out: Box<[u8; 4096]> = Box::new(unsafe { MaybeUninit::uninit().assume_init() });

        let offset = calc_offset(page_addr);

        self.file
            .read_exact_at(out.as_mut_slice(), offset)
            .expect("todo");

        DiskLeaf { inner: out }
    }

    /// Write the page of the given address
    pub fn write_page(&self, page_addr: u64, leaf: &DiskLeaf) {
        self.file
            .write_at(leaf.inner.as_slice(), calc_offset(page_addr))
            .expect("todo");
    }

    pub fn get_new_addr(&self) -> u64 {
        todo!()
    }
}

fn calc_offset(page_addr: u64) -> u64 {
    // add one for a metadata page
    let offset = (page_addr + 1) * 4096;
    offset
}
pub struct DiskLeaf {
    inner: Box<[u8; 4096]>,
}

impl DiskLeaf {
    pub fn as_ref(&self) -> &NodeMeta {
        unsafe { &*(self.inner.as_ptr() as *const NodeMeta) }
    }

    pub fn as_mut(&mut self) -> &mut NodeMeta {
        unsafe { &mut *(self.inner.as_ptr() as *mut NodeMeta) }
    }
}
