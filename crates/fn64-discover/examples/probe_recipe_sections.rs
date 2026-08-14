//! Print each overlay recipe's text/data/bss extents.
//!
//! WM2000's generation digests the WHOLE image, but the guest legitimately
//! writes its bss at runtime -- a 4-byte CPU store at +0x2636c is what breaks
//! the activation. If the recipe already separates text from bss, the digest
//! can cover only the immutable part.
fn main() {
    let path = std::env::args().nth(1).expect("usage: <rom.z64>");
    let bytes = std::fs::read(&path).expect("read rom");
    let rom = fn64_discover::rom::normalize(&bytes).expect("normalize");
    let recovery = fn64_discover::overlay_regions::recover_overlay_regions(
        &rom.bytes,
        &fn64_discover::overlay_regions::SearchConfig::aki_family(),
        &fn64_discover::delta_vote::DeltaVoteConfig::default(),
        1,
    );
    match fn64_discover::overlay_recipe::admitted_overlay_load_recipes_v1(&rom.bytes, &recovery) {
        Ok(recipes) => {
            for (index, recipe) in recipes.iter().enumerate() {
                println!(
                    "overlay {index}: load={:#010x} text=[{:#010x},{:#010x}) data_end={:#010x} bss_end={:#010x}",
                    recipe.load_start,
                    recipe.text_start,
                    recipe.text_end,
                    recipe.data_end,
                    recipe.bss_end,
                );
                println!(
                    "    text len={:#x}  data len={:#x}  bss len={:#x}",
                    recipe.text_end - recipe.text_start,
                    recipe.data_end - recipe.text_end,
                    recipe.bss_end - recipe.data_end,
                );
                println!(
                    "    loaded_sha256={}...  text_sha256={}...  differ={}",
                    &recipe.loaded_sha256[..16],
                    &recipe.text_sha256[..16],
                    recipe.loaded_sha256 != recipe.text_sha256,
                );
            }
        }
        Err(error) => println!("recipes unavailable: {error:?}"),
    }
}
