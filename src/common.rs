pub fn alloc_layout_buffers(
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
pub fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}
