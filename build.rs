fn main() {
  if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
    println!("cargo:rerun-if-changed=installer/wish.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("installer/wish.ico");
    resource.set("FileDescription", "Wish - self-hosted agent desktop app");
    resource.set("ProductName", "Wish");
    resource.compile().expect("embed the application icon");
  }
}
