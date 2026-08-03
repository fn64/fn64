fn main() {
    let bytes = std::fs::read("/Users/jer/Code/aki-recomp/donors/wcw-nwo-revenge.z64").unwrap();
    for (name, s, e, d) in [
        ("ovl_a", 0x3c770u32, 0x834a0u32, 0x80090000u32),
        ("ovl_b", 0x834a0, 0xdac50, 0x80090000),
    ] {
        let region = &bytes[s as usize..e as usize];
        let r = fn64_discover::overlay_regions::descriptor_mapping_corroborated_probe(region, s, e, d);
        println!("{name}: len={:#x} -> {:?}", e - s, r);
    }
}
