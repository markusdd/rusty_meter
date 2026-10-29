fn main() {
    if let Err(err) = rusty_meter::mcp_stdio::run() {
        eprintln!("rusty-meter-mcp: {err}");
        std::process::exit(1);
    }
}
