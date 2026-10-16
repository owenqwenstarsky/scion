use std::process::Command;

#[test]
fn binary_refuses_to_start_outside_a_container() {
    if std::path::Path::new("/.dockerenv").exists() {
        return;
    }
    let mut exe = std::env::current_exe().unwrap();
    exe.pop();
    exe.pop();
    exe.push("scion-server");
    let output = Command::new(exe).output().unwrap();
    assert_eq!(output.status.code(), Some(78));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("scion server up"), "{text}");
}
