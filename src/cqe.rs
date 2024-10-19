use crate::{buf_ring::BufRingPool, read_buf::KernelBuffer, sys as libc, UserData};

pub struct Completion {
    pub(crate) ptr: *const libc::io_uring_cqe,
    pub(crate) kernel_buf: Option<KernelBuffer>,
    pub(crate) user_data: UserData,
}

impl Completion {
    pub unsafe fn from_raw_event(ring: &mut BufRingPool, ptr: *const libc::io_uring_cqe) -> Self {
        assert!(!ptr.is_null());
        let cqe = &*ptr;
        let user_data = UserData::from_packed(cqe.user_data);

        if cqe.res < 0 {
            let os_err = std::io::Error::from_raw_os_error(cqe.res.abs());
            panic!("cqe error: {os_err}");
        }

        //assert_eq!(
        //    cqe.flags & libc::IORING_CQE_F_MORE,
        //    0,
        //    "unhandled IORING_CQE_F_MORE"
        //);
        assert_eq!(
            cqe.flags & libc::IORING_CQE_F_BUF_MORE,
            0,
            "unhandled IORING_CQE_F_BUF_MORE"
        );

        // data_len does NOT equal to buffer_len
        // buffer_len is always determined when first added to kernel ring
        // data_len is returned from cqe op result, e.g. how many bytes read/written
        let kernel_buf = if cqe.flags & libc::IORING_CQE_F_BUFFER != 0 {
            let data_len = cqe.res as usize;
            assert!(data_len > 0);

            // get buffer data
            let buffer_id = (cqe.flags >> libc::IORING_CQE_BUFFER_SHIFT) as u16;

            log::info!("[{}] completion require {}", user_data.owner(), buffer_id);

            // FIXME: this is a hack that we get buffer group id from user data
            let buffer_group_id = user_data.owner();
            let kernel_buf = ring.get_read_buf(buffer_group_id, buffer_id, data_len);
            assert_eq!(kernel_buf.bid, buffer_id);
            Some(kernel_buf)
        } else {
            None
        };

        Self {
            ptr,
            kernel_buf,
            user_data,
        }
    }

    // TODO: expose a proper interface to access fields within the raw cqe
    pub fn as_cqe_ref(&self) -> &libc::io_uring_cqe {
        unsafe { &*self.ptr }
    }

    /// take ownership of the attached kernel buffer
    /// caller is responsible to return kernel buffer ownership to kernel once this is taken
    pub fn take_kernel_buf(&mut self) -> Option<KernelBuffer> {
        self.kernel_buf.take()
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        // if there is an attached kernel_buffer it MUST be owned by some handler and do the proper
        // dropping
        assert!(self.kernel_buf.is_none(), "no one has consumed the kernel buffer and thus no one yield back the ownership to kernel")
    }
}
