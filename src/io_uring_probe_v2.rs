use log::info;
use tungstenite::protocol::WebSocketContext;

use crate::ring::{io_uring_buf_ring_add, Ring};
use crate::sys::{self as libc};
use crate::{op::*, Event, UserData};
use crate::{BGID, BUF_SIZE, RING_POOL_SIZE};
use core::panic;
use std::io;
use std::{
    ffi::CString,
    mem::MaybeUninit,
    ops::DerefMut,
    os::fd::RawFd,
    ptr,
    sync::{
        atomic::{self, AtomicU32},
        OnceLock,
    },
};

pub fn probe_sys_uring() {
    let mut ring = MaybeUninit::uninit();
    let ring_ptr: *mut libc::io_uring = ring.as_mut_ptr();

    // using liburing here to greatly reduce io_uring setup boilerplate
    let mut params: libc::io_uring_params = unsafe { std::mem::zeroed() };
    params.flags = libc::IORING_SETUP_SUBMIT_ALL;
    params.flags |= libc::IORING_SETUP_SQPOLL;
    // if not then set IORING_SETUP_COOP_TASKRUN
    params.flags |= libc::IORING_SETUP_SINGLE_ISSUER;

    let rval = unsafe { libc::io_uring_queue_init_params(1024, ring_ptr, &mut params) };
    if rval != 0 {
        let os_err = std::io::Error::last_os_error();
        panic!("{os_err}");
    }
    assert!(params.flags & libc::IORING_FEAT_SQPOLL_NONFIXED != 0);

    let mut ring = unsafe { ring.assume_init() };

    // prep buffer ring
    let pool_size = RING_POOL_SIZE;
    let buf_size = BUF_SIZE;
    let buf_ring =
        unsafe { &mut *rust_io_uring_setup_buf_ring(&mut ring, pool_size, BGID).unwrap() };

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

    // ring wrapper instance
    let _ring = Ring { ring_ptr, buf_ring };

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

    for i in 0..bufs.len() {
        let buf = &mut bufs[i];
        let buf_addr = (buf as *mut _) as usize;
        let buf = unsafe { (&mut *buf).assume_init() };
        println!("{:?} {}", buf, buf_addr as usize);
    }

    // real logic goes here

    // prep read
    let mut handlers = vec![];
    let mut streams = vec![];
    for _ in 0..1 {
        let (fd, ws) = init_tcp_stream(_ring.clone());
        let handler =
            WebSocketContext::new(tungstenite::protocol::Role::Client, Default::default());
        handlers.push(handler);
        streams.push(ws);
        //let fd = stream.as_raw_fd();

        //read_at(unsafe { &mut *sqe }, fd, ptr::null_mut(), buf_size, 0);

        let sqe = io_uring_get_sqe(&mut ring).unwrap();
        let user_data = UserData::from_parts(Event::MultishortRecv as _, 0, 1201);
        unsafe { (&mut *sqe).user_data = user_data.as_u64() };
        //recv(unsafe { &mut *sqe }, fd, ptr::null_mut(), 0, 0);
        multishot_recv(unsafe { &mut *sqe }, fd, 0, BGID);
    }

    let cqe_buf = [const { MaybeUninit::<libc::io_uring_cqe>::uninit() }; 10];
    // NOTE: assume_init is not stable as a const fn yet
    let mut cqe_buf = cqe_buf
        .into_iter()
        .map(|mut v| unsafe { v.assume_init_mut() as *mut _ })
        .collect::<Vec<_>>();

    loop {
        // submit is still required event with SQPOLL
        // io_uring_submit perform a __io_uring_flush_sq to sync kernal state for us
        // it also impose a write barrier to make sure kernal doesn't read a half written sqe
        // NOTE: io_uring_submit_and_wait(ring, n) seem to be always firing syscall though
        // despite doc saying it's not. needa check in liburing
        let rval = unsafe { libc::io_uring_submit(&mut ring) };
        if rval < 0 {
            let os_err = std::io::Error::last_os_error();
            panic!("{os_err}");
        } else if rval > 0 {
            log::trace!("submitted {rval} entries");
        }
        // TODO: this need to be replaced by somewhat a variant of `static int _io_uring_get_cqe`
        // busy polling the cqe is bad
        let rval = unsafe { libc::io_uring_peek_batch_cqe(&mut ring, cqe_buf.as_mut_ptr(), 10) };
        //println!("io_uring_peek_batch_cqe returned {rval} io completion(s) filled");

        for cqe_idx in 0..rval {
            let cqe = cqe_buf[cqe_idx as usize];
            let cqe = unsafe { &mut *cqe };
            if cqe.res < 0 {
                let os_err = std::io::Error::from_raw_os_error(cqe.res.abs());
                panic!("cqe error: {os_err}");
            } else {
                assert!(cqe.res > 0);
                //let decoded = String::from_utf8_lossy(data);
                // decode ssl
                let user_data = UserData::from_packed(cqe.user_data);
                let ws = streams.get_mut(user_data.owner() as usize).unwrap();
                match ws.get_mut() {
                    tungstenite::stream::MaybeTlsStream::NativeTls(s) => {
                        let stream = s.get_mut();
                        unsafe { stream._on_cqe(cqe).unwrap() };

                        let handler = handlers.get_mut(user_data.owner() as usize).unwrap();
                        match handler.read_message_frame(s) {
                            Ok(Some(msg)) => log::info!("{msg}"),
                            Ok(None) => (),
                            Err(tungstenite::Error::Io(io_err)) => {
                                if io_err.kind() == io::ErrorKind::WouldBlock {
                                    // not enough data to complete a frame read
                                }
                            }
                            Err(err) => panic!("{err}"),
                        };
                    }
                    tungstenite::stream::MaybeTlsStream::Plain(s) => {
                        todo!()
                    }
                    _ => unimplemented!(),
                }
            }

            // yield back the buffer ownership to kernal
            // atomic::compiler_fence(atomic::Ordering::Release);
            //unsafe {
            //    io_uring_buf_ring_add(
            //        buf_ring,
            //        buf.addr as _,
            //        buf_size,
            //        buffer_id as _,
            //        io_uring_buf_ring_mask(pool_size) as _,
            //        0,
            //    )
            //};
            //unsafe { io_uring_buf_ring_advance(buf_ring, 1 as _) };
            unsafe { io_uring_cq_advance(&mut ring, 1) };
        }
    }

    // tear down
    // close(fd);
    //unsafe {
    //    libc::io_uring_queue_exit(&mut ring);
    //}
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

/// this is indeed a more concise interface compare to io_uring_cqe_seen(3)
/// which is very misleading to take an *mut io_uring_cqe as input
/// but under the hood only naively advance the cq by 1
unsafe fn io_uring_cq_advance(ring: &mut libc::io_uring, nr: u32) {
    let kernal_head = atomic::AtomicU32::from_ptr(ring.cq.khead);
    kernal_head.fetch_add(nr, atomic::Ordering::Release);
}

unsafe fn io_uring_buf_ring_advance(br: &mut libc::io_uring_buf_ring, count: u16) {
    let tail_addr = ptr::addr_of!((&mut br.__bindgen_anon_1.__bindgen_anon_1).tail) as *mut _;
    let tail = atomic::AtomicU16::from_ptr(tail_addr);
    tail.fetch_add(count, atomic::Ordering::Release);
}

fn io_uring_buf_ring_mask(ring_entries: u16) -> u16 {
    ring_entries - 1
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
            return Err(std::io::Error::from_raw_os_error(ret));
        }
        Ok(&mut *buf_ring)
    }
}

// normal syscalls

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

fn close(fd: RawFd) {
    unsafe { libc::close(fd as _) };
}

fn alloc_layout_buffers(
    pool_size: u16,
    buf_size: u32,
    page_size: usize,
) -> std::io::Result<std::alloc::Layout> {
    match std::alloc::Layout::from_size_align(pool_size as usize * buf_size as usize, page_size) {
        Ok(layout) => Ok(layout),
        // This will only fail if the size is larger then roughly
        // `isize::MAX - PAGE_SIZE`, which is a huge allocation.
        Err(_) => Err(std::io::ErrorKind::OutOfMemory.into()),
    }
}

/// Size of a single page, often 4096.
#[allow(clippy::cast_sign_loss)] // Page size shouldn't be negative.
fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// using tungstenite here is absolutely overkill
/// what we want is an easy way to perform websocket upgrade and handshake
/// having tungstenite WebSocket stream however is useful when we don't care to pay some cost of
/// syscall for sending message to websocket directly
fn init_tcp_stream(
    ring: Ring,
) -> (
    RawFd,
    tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<crate::net::TcpStream>>,
) {
    use tungstenite::{self, client_tls_with_config};

    static INSTALL_ONCE: OnceLock<bool> = OnceLock::new();
    INSTALL_ONCE.get_or_init(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
        true
    });
    //let (mut ws, response) = connect("ws://0.0.0.0:8765").unwrap();
    //let (mut ws, response) = connect("wss://ws.okx.com:8443/ws/v5/public").unwrap();

    let tcp_stream = std::net::TcpStream::connect("ws.okx.com:8443").unwrap();
    let (mut ws, response) = client_tls_with_config(
        "wss://ws.okx.com:8443/ws/v5/public",
        //tcp_stream,
        crate::net::TcpStream::new_from_std(tcp_stream, ring, 0),
        Default::default(),
        None,
    )
    .unwrap();
    info!("{response:?}");
    ws.send(
        r#"
        {
          "op": "subscribe",
          "args": [
            {
              "channel": "books",
              "instId": "BTC-USDT"
            }
          ]
        }
        "#
        .into(),
    )
    .unwrap();
    let msg = match ws.read() {
        Ok(msg) => msg.to_text().unwrap().to_string(),
        Err(_) => todo!(),
    };
    info!("{msg}");
    ws.flush().unwrap();

    let fd = match ws.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true).unwrap();
            stream.set_nodelay(true).unwrap();
            stream.as_raw_fd()
        }
        // NOTE: native-tls is tough to work with due to the fact that under the hood
        //  it performs (non)blocking IO read through ffi ssl_read(3) to native openssl
        //
        tungstenite::stream::MaybeTlsStream::NativeTls(stream) => {
            stream.get_mut().set_nonblocking(true).unwrap();
            stream.get_mut().set_nodelay(true).unwrap();
            stream.get_mut().as_raw_fd()
        }
        // NOTE: rustls in comparsion exposes the underlying ssl_reader which takes any chunk of
        //  bytes and perform the decoding for us. the IO is very loosly coupled with ssl stream decoder
        //  which is exactly what we want because is real IO is performed by io_uring
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream.get_mut().set_nonblocking(true).unwrap();
            stream.get_mut().set_nodelay(true).unwrap();
            stream.get_mut().as_raw_fd()
        }
        _ => unimplemented!(),
    };
    (fd, ws)
}
