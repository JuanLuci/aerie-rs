//! Prints a captured packet as Rust debug text.
//!
//! cargo run -p aerie-protocol --example dump -- <packet id> <payload file>

fn main() {
    let mut a = std::env::args().skip(1);
    let id: u32 = a.next().expect("packet id").parse().unwrap();
    let data = std::fs::read(a.next().expect("payload file")).unwrap();
    match aerie_protocol::proto::debug(id, &data) {
        Ok(Some(text)) => println!("{text}"),
        Ok(None) => eprintln!("no generated type for packet {id}"),
        Err(e) => eprintln!("decode failed: {e}"),
    }
}
