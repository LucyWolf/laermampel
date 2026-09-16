// Bettet unter Windows das Ampel-Icon und die Versionsinfos in die .exe ein.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Lärmampel")
            .set("FileDescription", "Lärmampel");
        if let Err(e) = res.compile() {
            panic!("Icon konnte nicht eingebettet werden: {e}");
        }
    }
}
