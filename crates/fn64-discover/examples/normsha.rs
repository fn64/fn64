//! Print a ROM's normalized SHA-256 -- the identity a boot context binds to.
fn main() {
    let path = std::env::args().nth(1).expect("usage: normsha <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    println!("{}  {}", rom.sha256, path);
}
