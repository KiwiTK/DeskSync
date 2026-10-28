fn main() {
    // 只在目标平台是 Windows 时嵌入图标和版本信息；其他平台直接跳过。
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/DeskSync.ico");
    res.set("ProductName", "DeskSync");
    res.set("FileDescription", "Share one keyboard, mouse and clipboard over LAN");
    res.set("CompanyName", "KiwiTK");
    res.set("LegalCopyright", "MIT Licensed");
    if let Err(e) = res.compile() {
        println!("cargo:warning=嵌入 Windows 图标失败: {e}");
    }
}
