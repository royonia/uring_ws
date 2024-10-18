use crate::{ring::buf_ring_add_advance, sys as libc};
use std::{collections::VecDeque, io::Read};

use crate::NonSendable;

pub struct KernelBuffer {
    /// pointer to the start of buffer data
    pub addr: *const u8,
    /// data length for read
    pub data_len: usize,
    /// pointer to buf_ring
    pub buf_ring: *mut libc::io_uring_buf_ring,
    /// buffer id
    pub bid: u16,
    /// buffer group id
    pub bgid: u16,
    /// buffer length allocated
    pub buf_len: u32,
    /// marker to make KernelBuffer is !Send and !Sync
    /// KernelBuffer not thread safe because each kernel buffer return its ownership back to kernel
    /// while dropping - this is not a thread safe operation
    pub __nonsendable: NonSendable,
}

impl KernelBuffer {
    fn data_len(&self) -> usize {
        self.data_len
    }

    fn data(&self, offset: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.addr.add(offset), self.data_len - offset) }
    }
    // TODO: mabe a ReadMutBuf variant is better
    pub unsafe fn data_mut(&self, data_len: usize) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.addr as *mut u8, data_len)
    }
}

impl Drop for KernelBuffer {
    fn drop(&mut self) {
        unsafe { buf_ring_add_advance(self) };
    }
}

macro_rules! unwrap_option {
    ($op:expr) => {
        match &$op {
            Some(v) => v,
            None => unreachable!(),
        }
    };
}
macro_rules! unwrap_option_mut {
    ($op:expr) => {
        match &mut $op {
            Some(v) => v,
            None => unreachable!(),
        }
    };
}

pub struct KernelBufferReader {
    buf: Option<KernelBuffer>,
    claimed: usize,
}

impl KernelBufferReader {
    pub fn new(buf: KernelBuffer) -> Self {
        Self {
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
        unwrap_option_mut!(self.buf)
    }

    #[inline(always)]
    pub fn buf_ref(&self) -> &KernelBuffer {
        unwrap_option!(self.buf)
    }
}
impl Drop for KernelBufferReader {
    fn drop(&mut self) {
        let buf = self
            .buf
            .take()
            .expect("buf must not be none before dropping");
        drop(buf);
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
    // TODO: replaced with a double ring buf
    buf_vec: VecDeque<KernelBufferReader>,
}

impl KernelBufferVecReader {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf_vec: VecDeque::with_capacity(capacity),
        }
    }
    pub fn push(&mut self, buf: KernelBuffer) {
        self.buf_vec.push_back(KernelBufferReader::new(buf));
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
