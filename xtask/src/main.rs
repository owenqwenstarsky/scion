//! Workspace checks that Cargo features cannot express.

use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cmd = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "check-layers".into());
    match cmd.as_str() {
        "check-layers" => {
            if let Err(err) = check_layers() {
                eprintln!("xtask: {err}");
                ExitCode::from(1)
            } else {
                println!("layer check ok");
                ExitCode::SUCCESS
            }
        }
        other => {
            eprintln!("unknown command {other}");
            ExitCode::from(2)
        }
    }
}

fn check_layers() -> Result<(), String> {
    let forbidden = ["scion-store", "scion-derived", "scion-iam", "scion-server"];
    let clients = [
        "crates/scion-client/Cargo.toml",
        "crates/scion-vfs/Cargo.toml",
        "crates/scion-git-bridge/Cargo.toml",
        "crates/scion-cli/Cargo.toml",
    ];
    for path in clients {
        let text = fs::read_to_string(path).map_err(|err| format!("{path}: {err}"))?;
        let deps = dependency_block(&text);
        for name in forbidden {
            if deps
                .lines()
                .any(|line| line.split('=').next().unwrap_or("").trim() == name)
            {
                return Err(format!("{path} must not depend on {name}"));
            }
        }
    }
    Ok(())
}

fn dependency_block(text: &str) -> String {
    let mut out = String::new();
    let mut on = false;
    for line in text.lines() {
        if line.starts_with('[') {
            on = line == "[dependencies]";
            continue;
        }
        if on {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
