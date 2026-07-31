mod materializer;

fn main() {
    let root = std::env::var_os(materializer::PREPARED_ROOT_ENV).unwrap_or_else(|| {
        panic!(
            "prepared WM shard build requires {} to name a verifier-prepared private source tree",
            materializer::PREPARED_ROOT_ENV
        )
    });
    let root = std::path::PathBuf::from(root);
    let package = std::env::var("CARGO_PKG_NAME").expect("Cargo supplies CARGO_PKG_NAME");
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    materializer::emit_cargo_directives(&root, &package);
    materializer::materialize_package(&root, &package, &out)
        .unwrap_or_else(|error| panic!("materialize prepared WM shard {package}: {error}"));
}
