use crate::read_buf::KernelBufferVecReader;
use crate::ring::Ring;
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
    ring: Ring,
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
    pub fn new_from_std(raw: std::net::TcpStream, ring: Ring, owner_id: u16) -> Self {
        let inner_buf_size = RING_POOL_SIZE as usize;
        Self {
            inner: KernelBufferVecReader::new(ring, inner_buf_size),
            //inner: Cursor::new(vec![]),
            ring,
            owner_id,
            multishort_recv_id: 1201,
            send_id: AtomicU32::new(0),
            pending_sent: Default::default(),
            nonblocking: false,
            raw,
            __unsafe_mark: Default::default(),
        }
    }

    pub unsafe fn _on_cqe(&mut self, cqe: &libc::io_uring_cqe) -> Result<(), io::Error> {
        let user_data = UserData::from_packed(cqe.user_data);
        if user_data.owner() != self.owner_id {
            return Ok(());
        }

        match Event::from_u8(user_data.event()) {
            Send => {
                assert!(cqe.res != 0);

                let pending = self.pending_sent.get(&user_data).unwrap();
                assert_eq!(pending.len(), cqe.res as usize);
            }
            MultishortRecv => {
                log::info!(
                    "_on_cqe: req: {}, event: {}, res: {}",
                    user_data.req(),
                    user_data.event(),
                    cqe.res,
                );
                // using the entire user_data just for sqe_id is rather a waste
                assert_eq!(user_data.req(), self.multishort_recv_id);
                assert!(cqe.res != 0);
                assert!(cqe.flags & libc::IORING_CQE_F_BUFFER != 0);

                // data_len does NOT equal to buffer_len
                // buffer_len is always determined when first added to kernel ring
                // data_len is returned from cqe op result, e.g. how many bytes read/written
                let data_len = cqe.res as usize;
                assert!(data_len > 0);

                // get buffer data

                let buffer_id = (cqe.flags >> libc::IORING_CQE_BUFFER_SHIFT) as u16;

                let read_buf = self.ring.get_read_buf(buffer_id, data_len);
                // we are getting the exact buffer data
                //let data = read_buf.data_mut(data_len);
                //self.inner.get_mut().extend_from_slice(data);
                //self.ring.buf_ring_add_advance(read_buf);

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
            if len == 0 {
                Err(io::ErrorKind::WouldBlock.into())
            } else {
                Ok(len)
            }
            //log::info!("attempted cursor read {} bytes.", len);
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
