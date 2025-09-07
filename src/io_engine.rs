use std::{fs::File, mem::MaybeUninit, os::unix::fs::FileExt};

use crate::types::NodeMeta;

pub struct IoEngine {
    file: File,
}

impl IoEngine {
    pub fn get_page(&self, page_addr: u64) -> DiskLeaf {
        // SAFETY: this immediately overwritten
        let mut out: Box<[u8; 4096]> = Box::new(unsafe { MaybeUninit::uninit().assume_init() });

        // add one for a metadata page
        let offset = (page_addr + 1) * 4096;

        self.file
            .read_exact_at(out.as_mut_slice(), offset)
            .expect("todo");

        DiskLeaf { inner: out }
    }
}

pub struct DiskLeaf {
    inner: Box<[u8; 4096]>,
}

impl DiskLeaf {
    pub fn as_ref(&self) -> &NodeMeta {
        unsafe { &*(self.inner.as_ptr() as *const NodeMeta) }
    }
}
