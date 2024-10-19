use log::info;
use tungstenite::protocol::WebSocketContext;

use crate::buf_ring::BufRingPool;
use crate::cqe::Completion;
use crate::ring::{io_uring_get_sqe, Ring};
use crate::sys::{self as libc};
use crate::{op::*, Event, UserData, CQE_WAIT_NR, CQ_ENTRIES, MAX_BUFFER_GROUP};
use core::panic;
use std::io::{self, Write};
use std::time::Instant;
use std::{
    mem::MaybeUninit,
    os::fd::RawFd,
    ptr,
    sync::{
        atomic::{self},
        OnceLock,
    },
};

pub fn ping_pong() {
    let mut ring = MaybeUninit::uninit();

    // ring wrapper instance
    let _buf_ring = BufRingPool {
        buf_ring_ptrs: [unsafe { const { std::mem::zeroed() } }; MAX_BUFFER_GROUP],
    }
    .into_shared();

    // using liburing here to greatly reduce io_uring setup boilerplate
    let kernel_thread = false;
    let mut params: libc::io_uring_params = unsafe { std::mem::zeroed() };
    // submit all submissions on error
    // this should be by default enabled for most cases
    params.flags = libc::IORING_SETUP_SUBMIT_ALL;
    if kernel_thread {
        // sq_poll is conflict with coop_taskrun
        params.flags |= libc::IORING_SETUP_SQPOLL;
        //params.flags |= libc::IORING_SETUP_IOPOLL;
    } else {
        // if not then set IORING_SETUP_COOP_TASKRUN
        params.flags |= libc::IORING_SETUP_COOP_TASKRUN;
        params.flags |= libc::IORING_SETUP_DEFER_TASKRUN;
        params.flags |= libc::IORING_SETUP_SINGLE_ISSUER;
    }

    let rval = unsafe {
        libc::io_uring_queue_init_params(
            CQ_ENTRIES,
            std::ptr::addr_of_mut!(ring).cast::<libc::io_uring>(),
            &mut params,
        )
    };
    if rval != 0 {
        let os_err = std::io::Error::from_raw_os_error(rval.abs());
        panic!("{os_err}");
    }
    assert!(params.flags & libc::IORING_FEAT_SQPOLL_NONFIXED != 0);

    let mut ring = unsafe { ring.assume_init() };
    let _ring = Ring::from_raw_ptr(std::ptr::addr_of_mut!(ring));

    // real logic goes here

    // prep read
    let mut handlers = vec![];
    let mut streams = vec![];
    for owner_id in 0..1 {
        let recv_id = (owner_id + 1200) as u32;
        let (fd, ws) = init_tcp_stream(_ring, owner_id, recv_id);
        let handler =
            WebSocketContext::new(tungstenite::protocol::Role::Client, Default::default());
        handlers.push(handler);
        streams.push(ws);

        // init a new buffer group
        let buffer_group_id = owner_id;
        _buf_ring
            .borrow_mut()
            .new_buffer_group(&mut ring, buffer_group_id);

        let sqe = io_uring_get_sqe(&mut ring).unwrap();
        let user_data = UserData::from_parts(Event::MultishortRecv as _, owner_id, recv_id);
        unsafe { (&mut *sqe).user_data = user_data.as_u64() };
        //recv(unsafe { &mut *sqe }, fd, ptr::null_mut(), 0, 0);
        multishot_recv(unsafe { &mut *sqe }, fd, 0, buffer_group_id);
    }

    let cqe_buf = [const { MaybeUninit::<libc::io_uring_cqe>::uninit() }; 10];

    // NOTE: assume_init is not stable as a const fn yet
    let mut cqe_buf = cqe_buf
        .into_iter()
        .map(|mut v| unsafe { v.assume_init_mut() as *mut _ })
        .collect::<Vec<_>>();

    loop {
        for (stream, handler) in streams.iter_mut().zip(handlers.iter_mut()) {
            if handler.can_write() {
                handler.write(stream.get_mut(), "ping".into()).unwrap();
                handler.flush(stream.get_mut()).unwrap();
                log::info!("SENT PING");
            }
        }
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
            log::info!("submitted {rval} entries");
        }
        let rval = unsafe { libc::io_uring_peek_batch_cqe(&mut ring, cqe_buf.as_mut_ptr(), 10) };
        let to_ready = if rval == 0 {
            // if no immediate cqes then wait for the first one
            let ret = unsafe {
                libc::io_uring_wait_cqes(
                    &mut ring,
                    cqe_buf.as_mut_ptr(),
                    CQE_WAIT_NR,
                    ptr::null_mut() as _,
                    ptr::null_mut() as _,
                )
            };
            if ret < 0 {
                let os_err = std::io::Error::last_os_error();
                panic!("{os_err}");
            }
            1
        } else {
            rval
        };
        //println!("io_uring_peek_batch_cqe returned {rval} io completion(s) filled");

        // cqe
        for cqe_idx in 0..to_ready {
            let cqe = cqe_buf[cqe_idx as usize];
            let completion =
                unsafe { Completion::from_raw_event(&mut _buf_ring.borrow_mut(), cqe) };

            let stream_id = completion.user_data.owner();
            let ws = streams.get_mut(stream_id as usize).unwrap();
            match ws.get_mut() {
                tungstenite::stream::MaybeTlsStream::Plain(s) => {
                    unsafe { s.on_completion(completion).unwrap() };

                    let handler = handlers.get_mut(stream_id as usize).unwrap();
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
                _ => unimplemented!(),
            }

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

/// this is indeed a more concise interface compare to io_uring_cqe_seen(3)
/// which is very misleading to take an *mut io_uring_cqe as input
/// but under the hood only naively advance the cq by 1
unsafe fn io_uring_cq_advance(ring: &mut libc::io_uring, nr: u32) {
    let kernal_head = atomic::AtomicU32::from_ptr(ring.cq.khead);
    kernal_head.fetch_add(nr, atomic::Ordering::Release);
}
// normal syscalls

/// using tungstenite here is absolutely overkill
/// what we want from it is an easy way to perform ssl handshake and websocket upgrade
/// we also take the websocket frame decoder
fn init_tcp_stream(
    ring: Ring,
    owner_id: u16,
    multishot_recv_id: u32,
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
    let tcp_stream = std::net::TcpStream::connect("100.116.27.104:8765").unwrap();
    let (mut ws, response) = client_tls_with_config(
        "ws://100.116.27.104:8765",
        //tcp_stream,
        crate::net::TcpStream::new_from_std(tcp_stream, ring, owner_id, multishot_recv_id),
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
            },
            {
              "channel": "books",
              "instId": "ETH-USDT"
            },
            {
              "channel": "books",
              "instId": "SOL-USDT"
            },
            {
              "channel": "books",
              "instId": "BTC-USDT-SWAP"
            },
            {
              "channel": "books",
              "instId": "ETH-USDT-SWAP"
            },
            {
              "channel": "books",
              "instId": "SOL-USDT-SWAP"
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
