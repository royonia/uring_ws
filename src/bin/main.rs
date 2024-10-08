use ws_uring::io_uring_probe::probe_sys_uring;

fn main() {
    std::env::set_var("RUST_LOG", "DEBUG");
    env_logger::try_init().unwrap();
    probe_sys_uring();
}
