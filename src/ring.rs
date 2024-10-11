use std::{
    ops::DerefMut,
    ptr,
    sync::atomic::{self, AtomicU32},
};

use log::info;

use crate::{read_buf::KernelBuffer, sys as libc, RING_POOL_SIZE};

#[derive(Clone, Copy)]
pub struct Ring {
    pub(crate) ring_ptr: *mut libc::io_uring,
    pub(crate) buf_ring: *mut libc::io_uring_buf_ring,
}

impl Ring {
    pub fn get_sqe(&mut self) -> Result<*mut libc::io_uring_sqe, &'static str> {
        io_uring_get_sqe(self.ring_ref_mut())
    }

    pub unsafe fn get_read_buf(&mut self, buffer_id: u16, data_len: usize) -> KernelBuffer {
        //log::info!("try get_read_buf {buffer_id}");
        let data_buf = std::ptr::addr_of_mut!(self.buf_ring_ref_mut().__bindgen_anon_1.bufs)
            .cast::<libc::io_uring_buf>()
            .add(buffer_id as usize);
        debug_assert!(!data_buf.is_null());
        let data_buf = &*data_buf;

        debug_assert_eq!(
            data_buf.bid,
            buffer_id,
            "addr: {}",
            (data_buf as *const _) as usize
        );
        debug_assert!(data_buf.len >= data_len as u32);
        KernelBuffer {
            addr: data_buf.addr as *const u8,
            bid: data_buf.bid,
            buf_len: data_buf.len,
            data_len,
        }
    }

    pub unsafe fn buf_ring_add_advance(&mut self, read_buf: KernelBuffer) {
        log::info!("return ownership of buffer_{} to kernal", read_buf.bid);
        let KernelBuffer {
            addr, bid, buf_len, ..
        } = read_buf;
        io_uring_buf_ring_add(
            self.buf_ring_ref_mut(),
            addr,
            buf_len,
            bid,
            io_uring_buf_ring_mask(RING_POOL_SIZE) as _,
            0,
        );
        io_uring_buf_ring_advance(self.buf_ring_ref_mut(), 1);
    }

    fn ring_ref_mut(&mut self) -> &mut libc::io_uring {
        unsafe { &mut *self.ring_ptr }
    }

    fn buf_ring_ref(&self) -> &libc::io_uring_buf_ring {
        unsafe { &*self.buf_ring }
    }

    fn buf_ring_ref_mut(&mut self) -> &mut libc::io_uring_buf_ring {
        unsafe { &mut *self.buf_ring }
    }
}

fn io_uring_get_sqe(ring: &mut libc::io_uring) -> Result<*mut libc::io_uring_sqe, &'static str> {
    let sq = &mut ring.sq;
    let next = sq.sqe_tail.wrapping_add(1);
    let shift = 0;

    //atomic::fence(atomic::Ordering::SeqCst);
    let kernal_head = unsafe { AtomicU32::from_ptr(sq.khead) }.load(atomic::Ordering::Acquire);
    if next.wrapping_sub(kernal_head) <= sq.ring_entries {
        let sqe = unsafe {
            let idx = ((sq.sqe_tail & sq.ring_mask) << shift) as usize;
            //println!("get sqe at {idx}");
            sq.sqes.add(idx)
        };
        debug_assert!(!sqe.is_null());
        sq.sqe_tail = next as _;
        unsafe { io_uring_initialize_sqe(&mut *sqe) };
        Ok(sqe)
    } else {
        Err("sq ring entries overflow")
    }
}

unsafe fn io_uring_initialize_sqe(sqe: &mut libc::io_uring_sqe) {
    sqe.flags = 0;
    sqe.ioprio = 0;
    sqe.__bindgen_anon_3.rw_flags = 0;
    sqe.__bindgen_anon_4.buf_index = 0;
    sqe.personality = 0;
    sqe.__bindgen_anon_5.file_index = 0;
    sqe.__bindgen_anon_6.__bindgen_anon_1.deref_mut().addr3 = 0;
    sqe.__bindgen_anon_6.__bindgen_anon_1.deref_mut().__pad2[0] = 0;
}

pub(crate) unsafe fn io_uring_buf_ring_add(
    br: &mut libc::io_uring_buf_ring,
    addr: *const u8,
    len: u32,
    bid: u16,
    mask: u32,
    buf_offset: u32,
) {
    //struct io_uring_buf *buf = &br->bufs[(br->tail + buf_offset) & mask];
    let tail = br.__bindgen_anon_1.__bindgen_anon_1.tail as u32;
    let idx = (tail + buf_offset) & mask;
    let idx = idx as usize;
    let offset = size_of::<libc::io_uring_buf>();
    let buf = &mut *ptr::addr_of_mut!(br.__bindgen_anon_1.bufs)
        .cast::<libc::io_uring_buf>()
        .add(idx);
    buf.addr = (addr as usize) as libc::c_ulonglong;
    buf.len = len;
    buf.bid = bid;

    info!(
        "set buf[{}] ({}) to {} bid={}, offset={}",
        idx,
        (buf as *mut _) as u64,
        buf.addr,
        buf.bid,
        idx * offset,
    );
}
fn io_uring_buf_ring_mask(ring_entries: u16) -> u16 {
    ring_entries - 1
}

unsafe fn io_uring_buf_ring_advance(br: &mut libc::io_uring_buf_ring, count: u16) {
    let tail_addr = ptr::addr_of!((&mut br.__bindgen_anon_1.__bindgen_anon_1).tail) as *mut _;
    let tail = atomic::AtomicU16::from_ptr(tail_addr);
    tail.fetch_add(count, atomic::Ordering::Release);
}
