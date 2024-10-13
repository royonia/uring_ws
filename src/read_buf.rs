use std::{collections::VecDeque, io::Read};

use crate::ring::{Ring, SharedRing};

pub struct KernelBuffer {
    pub addr: *const u8,
    pub bid: u16,
    pub bgid: u16,
    pub data_len: usize,
    pub buf_len: u32,
}

impl KernelBuffer {
    fn data_len(&self) -> usize {
        self.data_len
    }

    fn data(&self, offset: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.addr.add(offset), self.data_len - offset) }
    }
}

impl KernelBuffer {
    // TODO: mabe a ReadMutBuf variant is better
    pub unsafe fn data_mut(&self, data_len: usize) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.addr as *mut u8, data_len)
    }
}

macro_rules! unwrap {
    ($op:expr) => {
        match &$op {
            Some(v) => v,
            None => unreachable!(),
        }
    };
}
macro_rules! unwrap_mut {
    ($op:expr) => {
        match &mut $op {
            Some(v) => v,
            None => unreachable!(),
        }
    };
}

pub struct KernelBufferReader {
    ring: SharedRing,
    buf: Option<KernelBuffer>,
    claimed: usize,
}

impl KernelBufferReader {
    pub fn new(ring: SharedRing, buf: KernelBuffer) -> Self {
        Self {
            ring,
            buf: Some(buf),
            claimed: 0,
        }
    }

    #[inline(always)]
    pub fn commit(&mut self, len: usize) {
        self.claimed += len;
        assert!(self.claimed <= self.buf_ref().data_len() as usize);
    }

    #[inline(always)]
    pub fn claimed(&self) -> usize {
        self.claimed
    }

    #[inline(always)]
    pub fn remaining(&self) -> usize {
        self.buf_ref().data_len() - self.claimed()
    }

    #[inline(always)]
    pub fn buf_ref_mut(&mut self) -> &mut KernelBuffer {
        unwrap_mut!(self.buf)
    }

    #[inline(always)]
    pub fn buf_ref(&self) -> &KernelBuffer {
        unwrap!(self.buf)
    }
}
impl Drop for KernelBufferReader {
    fn drop(&mut self) {
        let buf = self
            .buf
            .take()
            .expect("buf must not be none before dropping");
        unsafe { self.ring.borrow_mut().buf_ring_add_advance(buf) };
    }
}

impl Read for KernelBufferReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        assert!(self.remaining() > 0);
        //let claimed = self.buf_ref().data(self.claimed()).read(buf)?;
        let mut data = self.buf_ref().data(self.claimed());
        //log::info!("{data:?}");
        let claimed = data.read(buf)?;
        self.commit(claimed);
        log::trace!("buf[{}] claimed {} bytes", self.buf_ref().bid, claimed);
        assert!(self.claimed() <= self.buf_ref().data_len());
        Ok(claimed)
    }
}

pub struct KernelBufferVecReader {
    ring: SharedRing,
    buf_vec: VecDeque<KernelBufferReader>,
}

impl KernelBufferVecReader {
    pub fn new(ring: SharedRing, capacity: usize) -> Self {
        Self {
            ring,
            buf_vec: VecDeque::with_capacity(capacity),
        }
    }
    pub fn push(&mut self, buf: KernelBuffer) {
        self.buf_vec
            .push_back(KernelBufferReader::new(self.ring.clone(), buf));
        assert!(
            self.buf_vec.len() <= self.buf_vec.capacity(),
            "must not reallocate"
        );
    }
}

impl Read for KernelBufferVecReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut claimed = 0;
        let mut drained = false;
        if let Some(v) = self.buf_vec.get_mut(0) {
            assert!(v.remaining() > 0);

            //while v.remaining() > 0 {
            let bytes = v.read(&mut buf[claimed..])?;
            assert!(bytes > 0, "buf_len = {}", buf.len());
            claimed += bytes;

            drained = v.remaining() == 0;
            //}
        }

        if drained {
            let kernel_buf = self.buf_vec.pop_front().unwrap();
            log::trace!("trying to drop kernel_buf[{}]", kernel_buf.buf_ref().bid);
            drop(kernel_buf);
        }

        // clear drained buf
        Ok(claimed)
    }
}
