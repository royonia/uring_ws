use std::{ffi::CString, os::fd::RawFd};

use crate::sys as libc;

fn open_unchecked(path: impl AsRef<str>) -> RawFd {
    let fd = unsafe {
        let path = CString::from_vec_unchecked(
            path.as_ref().as_bytes().iter().copied().collect::<Vec<_>>(),
        );
        let fd = libc::open(path.as_ptr(), libc::O_RDONLY);
        fd as RawFd
    };
    fd as RawFd
}

#[inline]
fn close(fd: RawFd) {
    unsafe { libc::close(fd as _) };
}

#[inline]
fn read_at(sqe: &mut libc::io_uring_sqe, fd: RawFd, ptr: *mut u8, len: u32, offset: u64) {
    sqe.opcode = libc::IORING_OP_READ as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
    sqe.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
    sqe.len = len;
    // buffer select
    //sqe.__bindgen_anon_4.buf_group = BGID as u16;
    //sqe.flags |= libc::IOSQE_BUFFER_SELECT;
}

#[inline]
pub fn recv(sqe: &mut libc::io_uring_sqe, fd: RawFd, ptr: *mut u8, len: u32, flags: libc::c_int) {
    sqe.opcode = libc::IORING_OP_RECV as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
    sqe.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as _,
    };
    sqe.len = len;
    // buffer select
    //sqe.__bindgen_anon_4.buf_group = BGID as u16;
    //sqe.flags |= libc::IOSQE_BUFFER_SELECT;
}

/// Create a write submission starting at `offset`.
///
/// Avaialable since Linux kernel 5.6.
#[inline]
pub fn write_at(sqe: &mut libc::io_uring_sqe, fd: RawFd, ptr: *const u8, len: u32, offset: u64) {
    sqe.opcode = libc::IORING_OP_WRITE as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
    sqe.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
    sqe.len = len;
}

#[inline]
pub fn send(sqe: &mut libc::io_uring_sqe, fd: RawFd, ptr: *const u8, len: u32, flags: libc::c_int) {
    sqe.opcode = libc::IORING_OP_SEND as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
    sqe.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as _,
    };
    sqe.len = len;
}

#[inline]
pub fn multishot_recv(sqe: &mut libc::io_uring_sqe, fd: RawFd, flags: libc::c_int, buf_group: u16) {
    assert!(
        flags & libc::MSG_WAITALL == 0,
        "MSG_WAITALL cannot be set with multishot_recv"
    );
    sqe.opcode = libc::IORING_OP_RECV as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as _,
    };
    sqe.ioprio = libc::IORING_RECV_MULTISHOT as _;
    //sqe.ioprio |= libc::IORING_RECVSEND_POLL_FIRST as _;
    // buffer select
    sqe.__bindgen_anon_4.buf_group = buf_group as u16;
    sqe.flags |= libc::IOSQE_BUFFER_SELECT;
}
