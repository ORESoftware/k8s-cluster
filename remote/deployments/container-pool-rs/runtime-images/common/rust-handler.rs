use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    println!(
        "{{\"ok\":true,\"runtime\":\"rust\",\"receivedBytes\":{}}}",
        input.len()
    );
}
