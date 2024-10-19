use std::{
    ops::DerefMut,
    os::fd::RawFd,
    sync::atomic::{self, AtomicU32},
};

use crate::sys as libc;

use crate::{op, PhantomUnsend};

/// A trivially copiable / clonable & stateless ring wrapper
/// the constraint to fulfill is that ring must be used in single thread i.e. it is neither Send nor Sync
/// being !Send && !Sync is opiniated and nothing really stop us from passing the ring ptr around
/// fwiw ensure single_issuer is the only sensible way to get the most from io_uring
#[derive(Clone, Copy)]
pub struct Ring {
    ptr: *mut libc::io_uring,
    /// marker to ensure !Send & !Sync
    __marker_unsend: PhantomUnsend,
}

impl Ring {
    pub fn from_raw_ptr(ptr: *mut libc::io_uring) -> Self {
        Self {
            ptr,
            __marker_unsend: Default::default(),
        }
    }

    #[inline]
    pub fn _prep_send(
        &self,
        fd: RawFd,
        ptr: *const u8,
        len: usize,
        flags: i32,
    ) -> *mut libc::io_uring_sqe {
        let sqe = io_uring_get_sqe(self.as_ref_mut()).unwrap();
        op::send(unsafe { &mut *sqe }, fd, ptr, len as u32, flags);
        sqe
    }

    /// a mutable reference to the ring pointer itself doesn't take a mutable reference to `self`
    /// which is merely a wrapper
    #[inline]
    pub fn as_ref_mut(&self) -> &mut libc::io_uring {
        unsafe { &mut *self.ptr }
    }
}

#[inline]
pub fn io_uring_get_sqe(
    ring: &mut libc::io_uring,
) -> Result<*mut libc::io_uring_sqe, &'static str> {
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
        unsafe { io_uring_initialize_sqe(sqe) };
        Ok(sqe)
    } else {
        Err("sq ring entries overflow")
    }
}

/// bindings to liburing
/// IOURINGINLINE void io_uring_initialize_sqe(struct io_uring_sqe *sqe)
#[inline]
unsafe fn io_uring_initialize_sqe(sqe: *mut libc::io_uring_sqe) {
    let sqe = &mut *sqe;
    sqe.flags = 0;
    sqe.ioprio = 0;
    sqe.__bindgen_anon_3.rw_flags = 0;
    sqe.__bindgen_anon_4.buf_index = 0;
    sqe.personality = 0;
    sqe.__bindgen_anon_5.file_index = 0;
    sqe.__bindgen_anon_6.__bindgen_anon_1.deref_mut().addr3 = 0;
    sqe.__bindgen_anon_6.__bindgen_anon_1.deref_mut().__pad2[0] = 0;
}

/// bindings to liburing
/// IOURINGINLINE void io_uring_sqe_set_data(struct io_uring_sqe *sqe, void *data)
#[inline]
pub unsafe fn io_uring_sqe_set_data(sqe: *mut libc::io_uring_sqe, data: u64) {
    let sqe = &mut *sqe;
    sqe.user_data = data
}
