use std::{
    cell::RefCell,
    mem::MaybeUninit,
    ops::DerefMut,
    ptr,
    rc::Rc,
    sync::atomic::{self, AtomicU32},
};

use log::trace;

use crate::{
    common::*, read_buf::KernelBuffer, sys as libc, BufferGroupID, NonSendable, BUF_SIZE,
    MAX_BUFFER_GROUP, RING_POOL_SIZE,
};

pub type SharedRing = Rc<RefCell<BufRingPool>>;

pub struct BufRingPool {
    pub(crate) buf_ring_ptrs: [*mut libc::io_uring_buf_ring; MAX_BUFFER_GROUP],
}

impl BufRingPool {
    pub fn into_shared(self) -> SharedRing {
        Rc::new(RefCell::new(self))
    }

    // FIXME: we should definitely assert that buffer_group_id is not already occupied
    pub fn new_buffer_group(
        &mut self,
        ring: &mut libc::io_uring,
        buffer_group_id: BufferGroupID,
    ) -> &mut libc::io_uring_buf_ring {
        // prep buffer ring
        let pool_size = RING_POOL_SIZE;
        let buf_size = BUF_SIZE;
        let buf_ring = unsafe {
            &mut *rust_io_uring_setup_buf_ring(ring, pool_size, buffer_group_id).unwrap()
        };

        // allocate the real buffer
        // must be aligned
        let bufs_layout = alloc_layout_buffers(pool_size, buf_size, page_size()).unwrap();
        let bufs_addr = unsafe { std::alloc::alloc(bufs_layout) };
        if bufs_addr.is_null() {
            panic!("bufs_addr: out of memory");
        }

        let bufs = unsafe {
            std::slice::from_raw_parts_mut(
                std::ptr::addr_of_mut!(buf_ring.__bindgen_anon_1.bufs)
                    .cast::<MaybeUninit<libc::io_uring_buf>>(),
                pool_size as usize,
            )
        };

        for (i, _) in bufs.iter_mut().enumerate() {
            let addr = unsafe { bufs_addr.add(i * buf_size as usize) };
            //ring_buf.write(libc::io_uring_buf {
            //    addr: addr as u64,
            //    len: buf_size,
            //    bid: i as u16,
            //    resv: 0,
            //});
            //
            // this is essentially the same operation as writing directly into the ring_buf
            // because `bufs` is simply an allocated buffer based at addr(buf_ring) of length pool_size
            unsafe {
                io_uring_buf_ring_add(
                    buf_ring,
                    addr as _,
                    buf_size,
                    i as u16,
                    io_uring_buf_ring_mask(pool_size).into(),
                    i as u32,
                )
            }
        }
        unsafe {
            io_uring_buf_ring_advance(buf_ring, pool_size);
        }
        log::info!(
            "new_buffer_group {} at ({})",
            buffer_group_id,
            buf_ring as *const _ as usize
        );
        self.buf_ring_ptrs[buffer_group_id as usize] = buf_ring;
        buf_ring
    }

    pub unsafe fn get_read_buf(
        &mut self,
        buffer_group_id: BufferGroupID,
        buffer_id: u16,
        data_len: usize,
    ) -> KernelBuffer {
        let buf_ring = self.borrow_mut(buffer_group_id);
        log::info!("buf_ring at {}", buf_ring as *const _ as usize);
        log::info!(
            "try get_read_buf {buffer_id}. tail = {}",
            buf_ring.__bindgen_anon_1.__bindgen_anon_1.tail
        );
        let data_buf = std::ptr::addr_of!(buf_ring.__bindgen_anon_1.bufs)
            .cast::<libc::io_uring_buf>()
            .add(buffer_id as usize);
        debug_assert!(!data_buf.is_null());
        let data_buf = &*data_buf;

        if buffer_id != data_buf.bid {
            let bufs = unsafe {
                std::slice::from_raw_parts(
                    std::ptr::addr_of!(buf_ring.__bindgen_anon_1.bufs).cast::<libc::io_uring_buf>(),
                    RING_POOL_SIZE as usize,
                )
            };
            for i in 0..bufs.len() {
                let buf = &bufs[i];
                let buf_addr = (buf as *const _) as usize;
                log::info!("[{i}] {:?} {}", buf, buf_addr as usize);
            }

            log::error!(
                "expecting {}, got {}. \n
                data_buf_ptr: {}\n
                data_buf_addr: {}\n 
                bufs_ptr: {}\n
                tail: {}\n",
                buffer_id,
                data_buf.bid,
                (data_buf as *const _) as usize,
                data_buf.addr,
                std::ptr::addr_of!(buf_ring.__bindgen_anon_1.bufs).cast::<libc::io_uring_buf>()
                    as usize,
                buf_ring.__bindgen_anon_1.__bindgen_anon_1.tail,
            );
            panic!();
        }

        debug_assert!(data_buf.len >= data_len as u32);
        KernelBuffer {
            addr: data_buf.addr as *const u8,
            data_len,
            buf_ring: buf_ring as *mut _,
            bid: data_buf.bid,
            bgid: buffer_group_id,
            buf_len: data_buf.len,
            __nonsendable: Default::default(),
        }
    }

    #[inline(always)]
    fn borrow(&self, buffer_group_id: BufferGroupID) -> &libc::io_uring_buf_ring {
        unsafe { &*self.buf_ring_ptrs[buffer_group_id as usize] }
    }

    #[inline(always)]
    fn borrow_mut(&mut self, buffer_group_id: BufferGroupID) -> &mut libc::io_uring_buf_ring {
        unsafe { &mut *self.buf_ring_ptrs[buffer_group_id as usize] }
    }
}

#[inline(always)]
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

#[inline(always)]
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

#[inline(always)]
pub(crate) unsafe fn io_uring_buf_ring_add(
    br: &mut libc::io_uring_buf_ring,
    addr: *const u8,
    len: u32,
    bid: u16,
    mask: u32,
    buf_offset: u32,
) {
    //struct io_uring_buf *buf = &br->bufs[(br->tail + buf_offset) & mask];
    let tail_addr = ptr::addr_of!((&mut br.__bindgen_anon_1.__bindgen_anon_1).tail) as *mut _;
    let tail = atomic::AtomicU16::from_ptr(tail_addr).load(atomic::Ordering::Acquire) as u32;
    let idx = (tail + buf_offset) & mask;
    let idx = idx as usize;
    let offset = size_of::<libc::io_uring_buf>();
    let buf = &mut *ptr::addr_of_mut!(br.__bindgen_anon_1.bufs)
        .cast::<libc::io_uring_buf>()
        .add(idx);
    buf.addr = (addr as usize) as libc::c_ulonglong;
    buf.len = len;
    buf.bid = bid;

    trace!(
        "set buf[{}] ({}) to {} bid={}, offset={}",
        idx,
        (buf as *mut _) as u64,
        buf.addr,
        buf.bid,
        idx * offset,
    );
}

#[inline(always)]
fn io_uring_buf_ring_mask(ring_entries: u16) -> u16 {
    ring_entries - 1
}

#[inline(always)]
pub(crate) unsafe fn io_uring_buf_ring_advance(br: &mut libc::io_uring_buf_ring, count: u16) {
    let tail_addr = ptr::addr_of!((&mut br.__bindgen_anon_1.__bindgen_anon_1).tail) as *mut _;
    let tail = atomic::AtomicU16::from_ptr(tail_addr);
    tail.fetch_add(count, atomic::Ordering::Release);
}

#[inline(always)]
pub unsafe fn buf_ring_add_advance(read_buf: &mut KernelBuffer) {
    log::info!(
        "completion return ownership of buffer_{} to kernal",
        read_buf.bid
    );

    let KernelBuffer {
        buf_ring,
        addr,
        bid,
        buf_len,
        ..
    } = read_buf;
    let buf_ring = &mut **buf_ring;
    io_uring_buf_ring_add(
        buf_ring,
        *addr,
        *buf_len,
        *bid,
        io_uring_buf_ring_mask(RING_POOL_SIZE) as _,
        0,
    );
    io_uring_buf_ring_advance(buf_ring, 1);
}

/// binding to liburing io_uring_setup_buf_ring
/// https://man7.org/linux/man-pages/man3/io_uring_setup_buf_ring.3.html
fn rust_io_uring_setup_buf_ring(
    ring: *mut libc::io_uring,
    nentries: u16,
    bgid: u16,
) -> Result<*mut libc::io_uring_buf_ring, std::io::Error> {
    unsafe {
        let mut ret = 0i32;
        // flags is currently unused and must be set to zero.
        let buf_ring = libc::io_uring_setup_buf_ring(ring, nentries as _, bgid as _, 0, &mut ret);
        if buf_ring.is_null() {
            return Err(std::io::Error::from_raw_os_error(ret.abs()));
        }
        Ok(&mut *buf_ring)
    }
}
