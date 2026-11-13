fn main() {
    rupix_lib::platform::logging::install_logging();
    let img = rupix_lib::core::paths::test_scratch_root().join("rupix-mini3-target.txt");
    std::fs::write(&img, b"mini3").unwrap();
    println!("target = {}", img.display());
    std::process::Command::new("explorer")
        .arg(format!("/select,{img:?}"))
        .spawn()
        .ok();
    std::thread::sleep(std::time::Duration::from_millis(1800));
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};
        let fg = GetForegroundWindow();
        let mut buf = [0u16; 64];
        let n = GetClassNameW(fg, &mut buf);
        println!(
            "fg class = {:?}",
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        );
    }
    let got =
        rupix_lib::platform::shell_selection::foreground_selected_file(&|_: &std::path::Path| true);
    println!("take = {got:?}");
    std::fs::remove_file(&img).ok();
}
