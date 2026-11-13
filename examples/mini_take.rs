// 最小对照：install_decode_stack + explorer /select + 取件。不碰 spacehook。
fn main() {
    rupix_lib::install_decode_stack();
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.temp");
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join(format!("rupix-mini-target-{}.txt", std::process::id()));
    std::fs::write(&img, b"mini").unwrap();
    std::process::Command::new("explorer")
        .arg(format!("/select,{img:?}"))
        .spawn()
        .ok();
    std::thread::sleep(std::time::Duration::from_millis(1800));
    let got =
        rupix_lib::platform::shell_selection::foreground_selected_file(&|_: &std::path::Path| true);
    println!("no-hook take = {got:?}");
    std::fs::remove_file(&img).ok();
}
