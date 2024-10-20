use crate::cqe::Completion;
use crate::read_buf::KernelBufferVecReader;
use crate::ring::{io_uring_sqe_set_data, Ring};
use crate::{
    sys as libc,
    Event::{self, *},
    UserData,
};
use crate::{PhantomUnsend, RING_POOL_SIZE};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};

pub struct TcpStream {
    //inner: Cursor<Vec<u8>>,
    inner: KernelBufferVecReader,
    ring: Ring,
    // rings related
    owner_id: u16,
    multishort_recv_id: u32,
    send_id: u32,
    pending_sent: HashMap<UserData, Vec<u8>>,

    raw: std::net::TcpStream,
    nonblocking: bool,

    _marker_unsend: PhantomUnsend,
}

impl TcpStream {
    pub fn new_from_std(raw: std::net::TcpStream, ring: Ring, owner_id: u16, recv_id: u32) -> Self {
        let inner_buf_size = RING_POOL_SIZE as usize;
        Self {
            inner: KernelBufferVecReader::new(inner_buf_size),
            //inner: Cursor::new(vec![]),
            ring,
            owner_id,
            multishort_recv_id: recv_id,
            send_id: 1000,
            pending_sent: Default::default(),
            nonblocking: false,
            raw,
            _marker_unsend: Default::default(),
        }
    }

    /// take ownership of an completion event
    /// a completion is event is supposed to be handled by ONE owner only assigned by owner_id
    /// double consuming a completion event is unsafe because one might consume and mutate the
    /// content of the assigned kernel buffer
    /// and nothing stop one from returning the ownership of a kernel buffer back to kernel
    /// that we should avoid any handlers that sniffed the completion event and try to consume the
    /// kernel buffer
    pub unsafe fn on_completion(&mut self, mut completion: Completion) -> Result<(), io::Error> {
        let user_data = completion.user_data;
        assert_eq!(user_data.owner(), self.owner_id);

        match Event::from_u8(user_data.event()) {
            Send => {
                assert!(
                    completion.as_cqe_ref().res != 0,
                    "error: {}",
                    std::io::Error::from_raw_os_error(completion.as_cqe_ref().res.abs())
                );

                let pending = self.pending_sent.remove(&user_data).unwrap();
                // short send is in theory impossible with MSG_WAITALL flag set
                assert_eq!(
                    pending.len(),
                    completion.as_cqe_ref().res as usize,
                    "unhandled short send"
                );
                log::info!("handled Send event {}", user_data.data);
            }
            MultishortRecv => {
                assert_eq!(user_data.req(), self.multishort_recv_id);
                log::info!(
                    "_on_cqe: req: {}, event: {}, res: {}",
                    user_data.req(),
                    user_data.event(),
                    completion.as_cqe_ref().res,
                );
                // using the entire user_data just for sqe_id is rather a waste
                assert_eq!(user_data.req(), self.multishort_recv_id);
                assert!(
                    completion.as_cqe_ref().res != 0,
                    "error: {}",
                    std::io::Error::from_raw_os_error(completion.as_cqe_ref().res.abs())
                );
                assert!(completion.as_cqe_ref().flags & libc::IORING_CQE_F_BUFFER != 0);

                let read_buf = completion
                    .take_kernel_buf()
                    .expect("already asserted that kernel buff must exist");
                // append to inner
                self.inner.push(read_buf);

                //log::info!("_on_cqe read data: {:?}", self.inner);
            }
        };
        Ok(())
    }

    #[inline]
    pub fn as_raw_fd(&self) -> RawFd {
        self.raw.as_raw_fd()
    }

    #[inline]
    pub fn set_nonblocking(&mut self, v: bool) -> io::Result<()> {
        self.raw.set_nonblocking(v)?;
        self.nonblocking = v;
        Ok(())
    }

    #[inline]
    pub fn set_nodelay(&self, v: bool) -> io::Result<()> {
        self.raw.set_nodelay(v)
    }

    #[inline]
    fn next_send_id(&mut self) -> u32 {
        let ret = self.send_id;
        self.send_id = self.send_id.wrapping_add(1);
        ret
    }
}

impl Read for TcpStream {
    /// read is polled by multishot recv thus this read simply advance the inner buf cursor
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // TODO: mark branch `unlikely`
        if !self.nonblocking {
            return self.raw.read(buf);
        }
        let len = self.inner.read(buf)?;
        log::info!("attempted cursor read {} bytes.", len);
        if len == 0 {
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            Ok(len)
        }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        todo!()
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        todo!()
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        todo!()
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        todo!()
    }
}

impl Write for TcpStream {
    //fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    //    assert!(!self.nonblocking);
    //    self.raw.write(buf)
    //}
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // TODO: mark branch `unlikely`
        if !self.nonblocking {
            return self.raw.write(buf);
        }

        log::info!("attempt nonblock write {} bytes", buf.len());
        assert!(buf.len() > 0, "attempt to write an empty buffer");

        let copied = buf.to_vec();
        let data_ptr = copied.as_ptr();
        let data_len = copied.len();

        let mut flags = 0i32;
        // NOTE: whether this flag works on older kernel version (pre 6.0) is questionable
        flags |= libc::MSG_WAITALL;

        // TODO: use buffer_select
        // TODO: use op send_zc: https://lwn.net/Articles/900083/
        let sqe = self
            .ring
            ._prep_send(self.as_raw_fd(), data_ptr, data_len, flags);

        let user_data = UserData::from_parts(Send as _, self.owner_id, self.next_send_id());
        unsafe { io_uring_sqe_set_data(sqe, user_data.as_u64()) };

        self.pending_sent.insert(user_data, copied);
        Ok(data_len)
    }

    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        todo!()
    }

    fn write_fmt(&mut self, fmt: std::fmt::Arguments<'_>) -> io::Result<()> {
        todo!()
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        assert!(!self.nonblocking);
        self.raw.write_vectored(bufs)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        if !self.nonblocking {
            self.raw.flush()
        } else {
            Ok(())
        }
    }
}
