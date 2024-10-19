use ws_uring::{io_uring_probe_v2::probe_sys_uring, ping_pong::ping_pong};

fn main() {
    std::env::set_var("RUST_LOG", "INFO");
    env_logger::try_init().unwrap();
    ping_pong();
}
