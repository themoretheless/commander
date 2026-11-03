// Fixture producer for empty and known-size streams; verified by 7-Zip.
use std::io::{Read,Write};
fn main() {
    let mut data=Vec::new();std::io::stdin().read_to_end(&mut data).unwrap();
    let mut encoder=ppmd_rust::Ppmd8Encoder::new(Vec::new(),6,1024*1024,ppmd_rust::RestoreMethod::Restart).unwrap();
    encoder.write_all(&data).unwrap();
    let encoded=encoder.finish(!std::env::args().any(|a|a=="--no-eos")).unwrap();
    std::io::stdout().write_all(&5u16.to_le_bytes()).unwrap();std::io::stdout().write_all(&encoded).unwrap();
}
