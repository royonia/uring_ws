use std::os::fd::RawFd;

use crate::sys as libc;

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
pub fn write_at(sqe: &mut libc::io_uring_sqe, fd: RawFd, ptr: *const u8, len: u32, offset: u64) {
    sqe.opcode = libc::IORING_OP_WRITE as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
    sqe.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
    sqe.len = len;
}

pub fn send(sqe: &mut libc::io_uring_sqe, fd: RawFd, ptr: *const u8, len: u32, flags: libc::c_int) {
    sqe.opcode = libc::IORING_OP_SEND as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
    sqe.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as _,
    };
    sqe.len = len;
}

pub fn multishot_recv(sqe: &mut libc::io_uring_sqe, fd: RawFd, flags: libc::c_int, buf_group: u16) {
    sqe.opcode = libc::IORING_OP_RECV as u8;
    sqe.fd = fd;
    sqe.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as _,
    };
    sqe.ioprio = libc::IORING_RECV_MULTISHOT as _;
    // buffer select
    sqe.__bindgen_anon_4.buf_group = buf_group as u16;
    sqe.flags |= libc::IOSQE_BUFFER_SELECT;
}
