fn main() {
    rupix_lib::platform::logging::install_logging();
    let img = rupix_lib::core::paths::test_scratch_root().join("rupix-mini-target.txt");
    std::fs::write(&img, b"mini").unwrap();
    std::process::Command::new("explorer")
        .arg(format!("/select,{img:?}"))
        .spawn()
        .ok();
    std::thread::sleep(std::time::Duration::from_millis(1800));
    let got =
        rupix_lib::platform::shell_selection::foreground_selected_file(&|_: &std::path::Path| true);
    println!("take = {got:?}");
    std::fs::remove_file(&img).ok();
}
