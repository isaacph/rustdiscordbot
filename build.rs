use std::path::Path;

fn main() {
    let path = std::path::absolute(Path::new("res/cacert.pem")).unwrap();
    println!("cargo::rerun-if-changed={}", path.to_str().unwrap());
    println!("cargo::rustc-env=CACERTS_PATH={}", path.to_str().unwrap());
}
