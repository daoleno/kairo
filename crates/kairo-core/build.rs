fn main() {
    // The catalog (`rust_i18n::i18n!`) is read when the crate builds.
    println!("cargo:rerun-if-changed=../../locales");
}
