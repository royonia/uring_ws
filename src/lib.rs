use std::fmt::Display;

mod buf_ring;
mod common;
mod cqe;
mod net;
mod op;
mod read_buf;
mod ring;
mod sys;

pub mod io_uring_probe_v2;
pub mod ping_pong;

pub type OwnID = u16;
pub type BufferGroupID = u16;

pub const MAX_BUFFER_GROUP: usize = 256;

pub const CQ_ENTRIES: u32 = 1024;
/// we might actually want a buffer group per connection?
pub const BGID: u16 = 0;
/// large enough to fit one datagram
pub const BUF_SIZE: u32 = 1500;
/// rather random. increase if sqe submission are backpressured
pub const RING_POOL_SIZE: u16 = 2u16.pow(10);

// allocated_mem_size_per_buffer = 1024 * 1500 = ~1MB
// max_allocated_mem_size = 1024 * 1500 * 256 = ~300MB

pub const CQE_WAIT_NR: u32 = 1;

pub type PhantomUnsend = std::marker::PhantomData<*const ()>;

#[repr(u8)]
enum Event {
    Send = 0,
    MultishortRecv = 1,
}
impl Event {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Send,
            1 => Self::MultishortRecv,
            _ => panic!("unknown event from userdata"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct UserData {
    data: u64,
}

impl Display for UserData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UserData({})", self.data)
    }
}

impl UserData {
    fn from_packed(data: u64) -> Self {
        Self { data }
    }
    fn from_parts(event: u8, owner: u16, req: u32) -> Self {
        let mut data = 0u64;

        // Pack event (u8) into the upper 8 bits of the u64
        data |= (event as u64) << 56;

        // Pack owner (u16) into the next 16 bits after the event
        data |= (owner as u64) << 40;

        // Pack req (u32) into the lower 32 bits of the u64
        data |= req as u64;

        UserData { data }
    }
    fn as_u64(&self) -> u64 {
        self.data
    }

    // Getter to unpack and retrieve the event (u8)
    fn event(&self) -> u8 {
        (self.data >> 56) as u8
    }

    // Getter to unpack and retrieve the owner (u16)
    fn owner(&self) -> u16 {
        ((self.data >> 40) & 0xFFFF) as u16
    }

    // Getter to unpack and retrieve the req (u32)
    fn req(&self) -> u32 {
        (self.data & 0xFFFFFFFF) as u32
    }
}

impl From<UserData> for u64 {
    fn from(value: UserData) -> Self {
        value.data
    }
}
