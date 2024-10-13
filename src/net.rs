use crate::cqe::Completion;
use crate::read_buf::KernelBufferVecReader;
use crate::ring::{Ring, SharedRing};
use crate::RING_POOL_SIZE;
use crate::{
    sys as libc,
    Event::{self, *},
    UserData, BUF_SIZE,
};
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Write};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::AtomicU32;

pub struct TcpStream {
    //inner: Cursor<Vec<u8>>,
    inner: KernelBufferVecReader,
    ring: SharedRing,
    // rings related
    owner_id: u16,
    multishort_recv_id: u32,
    send_id: AtomicU32,
    pending_sent: HashMap<UserData, Vec<u8>>,

    raw: std::net::TcpStream,
    nonblocking: bool,

    __unsafe_mark: PhantomData<()>,
}

impl TcpStream {
    pub fn new_from_std(
        raw: std::net::TcpStream,
        ring: SharedRing,
        owner_id: u16,
        recv_id: u32,
    ) -> Self {
        let inner_buf_size = RING_POOL_SIZE as usize;
        Self {
            inner: KernelBufferVecReader::new(ring.clone(), inner_buf_size),
            //inner: Cursor::new(vec![]),
            ring,
            owner_id,
            multishort_recv_id: recv_id,
            send_id: AtomicU32::new(0),
            pending_sent: Default::default(),
            nonblocking: false,
            raw,
            __unsafe_mark: Default::default(),
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
        if user_data.owner() != self.owner_id {
            return Ok(());
        }

        match Event::from_u8(user_data.event()) {
            Send => {
                assert!(completion.raw_cqe().res != 0);

                let pending = self.pending_sent.get(&user_data).unwrap();
                assert_eq!(pending.len(), completion.raw_cqe().res as usize);
            }
            MultishortRecv => {
                assert_eq!(user_data.req(), self.multishort_recv_id);
                log::info!(
                    "_on_cqe: req: {}, event: {}, res: {}",
                    user_data.req(),
                    user_data.event(),
                    completion.raw_cqe().res,
                );
                // using the entire user_data just for sqe_id is rather a waste
                assert_eq!(user_data.req(), self.multishort_recv_id);
                assert!(completion.raw_cqe().res != 0);
                assert!(completion.raw_cqe().flags & libc::IORING_CQE_F_BUFFER != 0);

                let read_buf = completion
                    .take_kernel_buf()
                    .expect("already asserted that kernel buff must exist");
                // append to inner
                self.inner.push(read_buf);

                //log::info!("_on_cqe read data: {:?}", self.inner);

                // yield back buffer to kernal
            }
        };
        Ok(())
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.raw.as_raw_fd()
    }

    pub fn set_nonblocking(&mut self, v: bool) -> io::Result<()> {
        self.raw.set_nonblocking(v)?;
        self.nonblocking = v;
        Ok(())
    }

    pub fn set_nodelay(&self, v: bool) -> io::Result<()> {
        self.raw.set_nodelay(v)
    }
}

impl Read for TcpStream {
    /// read is polled by multishot recv thus this read simply advance the inner buf cursor
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.nonblocking {
            let len = self.inner.read(buf)?;
            log::info!("attempted cursor read {} bytes.", len);
            if len == 0 {
                Err(io::ErrorKind::WouldBlock.into())
            } else {
                Ok(len)
            }
        } else {
            self.raw.read(buf)
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
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        assert!(!self.nonblocking);
        self.raw.write(buf)
    }
    //fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    //    log::info!("write request: {:?}", buf);
    //    // get a sqe and prep a write op
    //    let sqe = unsafe { &mut *self.ring.get_sqe().unwrap() };
    //    let copied = buf.to_vec();
    //
    //    // theres no way to know ahead how much data are sent SUCCESSFULLY
    //    send(sqe, self.as_raw_fd(), copied.as_ptr(), copied.len() as _, 0);
    //    let user_data = UserData::from_parts(
    //        Send as _,
    //        self.owner_id,
    //        self.send_id.fetch_add(1, atomic::Ordering::Relaxed),
    //    );
    //
    //    self.pending_sent.insert(user_data, copied);
    //    Ok(buf.len())
    //}
    //

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

    fn flush(&mut self) -> io::Result<()> {
        if !self.nonblocking {
            self.raw.flush()
        } else {
            Ok(())
        }
    }
}
