//! Host CLI. `scion server` shells out to Docker Compose. It does not link
//! the server binary or open the database.

use clap::{Parser, Subcommand};
use scion_client::{diff_index, scion_home, Client, SparseSpec, Workspace};
use scion_core::BlobId;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "scion",
    about = "Local-first client for a Scion server running in Docker"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Store a session for later commands.
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },
    /// Create or list repositories you can see.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Materialize owner/name into the current directory.
    Clone {
        spec: String,
    },
    Status,
    /// Upload a changeset. `scion land` publishes it.
    Commit {
        #[arg(short, long)]
        message: String,
    },
    Land {
        #[arg(long, default_value = "rebase")]
        strategy: String,
    },
    Update,
    Log {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    Blame {
        path: String,
    },
    Sparse {
        #[command(subcommand)]
        cmd: SparseCmd,
    },
    Doctor,
    Token {
        #[command(subcommand)]
        cmd: TokenCmd,
    },
    /// Docker lifecycle for the server. The CLI never starts the server in-process.
    Server {
        #[command(subcommand)]
        cmd: ServerCmd,
    },
}

#[derive(Subcommand)]
enum AuthCmd {
    Login {
        #[arg(long)]
        server: String,
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        insecure: bool,
    },
    Logout,
    Whoami,
}

#[derive(Subcommand)]
enum RepoCmd {
    List,
    Create {
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
}

#[derive(Subcommand)]
enum SparseCmd {
    Show,
    Set { spec: String },
}

#[derive(Subcommand)]
enum TokenCmd {
    Create {
        name: String,
        #[arg(long, default_value = "write")]
        max_role: String,
    },
}

#[derive(Subcommand)]
enum ServerCmd {
    Up,
    Down,
    Logs,
    Status,
    Restart,
    Backup { dest: PathBuf },
    Restore { src: PathBuf },
}

#[tokio::main]
async fn main() {
    if let Err(err) = run(Cli::parse()).await {
        eprintln!("scion: {err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Auth { cmd } => auth(cmd).await,
        Cmd::Repo { cmd } => repo(cmd).await,
        Cmd::Clone { spec } => clone_repo(&spec).await,
        Cmd::Status => status().await,
        Cmd::Commit { message } => commit(&message).await,
        Cmd::Land { strategy } => land(&strategy).await,
        Cmd::Update => update().await,
        Cmd::Log { limit } => log(limit).await,
        Cmd::Blame { path } => blame(&path).await,
        Cmd::Sparse { cmd } => sparse(cmd),
        Cmd::Doctor => doctor().await,
        Cmd::Token { cmd } => token(cmd).await,
        Cmd::Server { cmd } => server(cmd).await,
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthFile {
    server: String,
    username: String,
    session_token: String,
    csrf: String,
    insecure: bool,
}

fn auth_path() -> PathBuf {
    scion_home().join("auth.json")
}

fn load_auth() -> Result<AuthFile, String> {
    let text = fs::read_to_string(auth_path())
        .map_err(|_| "not logged in; run scion auth login".to_string())?;
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

fn client_from_auth(auth: &AuthFile) -> Result<Client, String> {
    Ok(Client::new(&auth.server, auth.insecure)
        .map_err(|err| err.to_string())?
        .with_session(&auth.session_token, &auth.csrf))
}

async fn auth(cmd: AuthCmd) -> Result<(), String> {
    match cmd {
        AuthCmd::Login {
            server,
            username,
            password,
            insecure,
        } => {
            let client = Client::new(&server, insecure).map_err(|err| err.to_string())?;
            let value = client
                .login(&username, &password)
                .await
                .map_err(|err| err.to_string())?;
            let file = AuthFile {
                server,
                username: value["username"].as_str().unwrap_or(&username).to_string(),
                session_token: value["session_token"].as_str().unwrap_or("").to_string(),
                csrf: value["csrf_token"].as_str().unwrap_or("").to_string(),
                insecure,
            };
            let path = auth_path();
            fs::create_dir_all(path.parent().unwrap()).map_err(|err| err.to_string())?;
            fs::write(&path, serde_json::to_vec_pretty(&file).unwrap())
                .map_err(|err| err.to_string())?;
            println!("logged in as {}", file.username);
            if value["must_change_password"] == true {
                println!("password change required before other commands");
            }
            Ok(())
        }
        AuthCmd::Logout => {
            let _ = fs::remove_file(auth_path());
            println!("logged out");
            Ok(())
        }
        AuthCmd::Whoami => {
            let auth = load_auth()?;
            let client = client_from_auth(&auth)?;
            let me = client.session().await.map_err(|err| err.to_string())?;
            println!("{}", serde_json::to_string_pretty(&me).unwrap());
            Ok(())
        }
    }
}

async fn repo(cmd: RepoCmd) -> Result<(), String> {
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    match cmd {
        RepoCmd::List => {
            let repos = client.list_repos().await.map_err(|err| err.to_string())?;
            println!("{}", serde_json::to_string_pretty(&repos).unwrap());
            Ok(())
        }
        RepoCmd::Create { name, description } => {
            let created = client
                .create_repo(&name, &description)
                .await
                .map_err(|err| err.to_string())?;
            println!(
                "created {}/{}",
                created["owner"].as_str().unwrap_or(&auth.username),
                name
            );
            Ok(())
        }
    }
}

async fn clone_repo(spec: &str) -> Result<(), String> {
    let (owner, name) =
        scion_client::repo_spec(spec).ok_or_else(|| format!("expected owner/name, got {spec}"))?;
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    let info = client
        .get_repo(owner, name)
        .await
        .map_err(|err| err.to_string())?;
    let ref_name = info["default_ref"].as_str().unwrap_or("main").to_string();
    let ws = Workspace {
        server: auth.server,
        owner: owner.to_string(),
        name: name.to_string(),
        ref_name,
        tip: None,
        pending: None,
    };
    ws.save(Path::new("."))?;
    update().await
}

fn workspace() -> Result<Workspace, String> {
    Workspace::load(Path::new("."))
}

async fn status() -> Result<(), String> {
    let ws = workspace()?;
    let index = Workspace::load_index(Path::new("."))?;
    let sparse = Workspace::load_sparse(Path::new("."))?;
    let local = hash_worktree(".")?;
    let lines = diff_index(&index, &local, &sparse);
    if lines.is_empty() {
        println!(
            "clean {}/{} @ {}",
            ws.owner,
            ws.name,
            ws.tip.as_deref().unwrap_or("unborn")
        );
    }
    for line in lines {
        println!("{:8} {}", line.state, line.path);
    }
    Ok(())
}

fn hash_worktree(root: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    walk_files(Path::new(root), Path::new(root), &mut out)?;
    Ok(out)
}

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".scion" || name == ".git" || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            walk_files(root, &path, out)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).unwrap();
            let rel = rel.to_string_lossy().replace('\\', "/");
            let bytes = fs::read(&path).map_err(|err| err.to_string())?;
            out.push((rel, BlobId::of(&bytes).to_hex()));
        }
    }
    Ok(())
}

async fn commit(message: &str) -> Result<(), String> {
    let mut ws = workspace()?;
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    let sparse = Workspace::load_sparse(Path::new("."))?;
    let index = Workspace::load_index(Path::new("."))?;
    let local = hash_worktree(".")?;
    let lines = diff_index(&index, &local, &sparse);
    if lines.is_empty() {
        return Err("nothing to commit".into());
    }
    let mut changes = Vec::new();
    for line in &lines {
        if line.state == "deleted" {
            changes.push(json!({"path": line.path, "op": "delete"}));
            continue;
        }
        let bytes = fs::read(&line.path).map_err(|err| err.to_string())?;
        let id = client
            .put_blob(&ws.owner, &ws.name, &bytes)
            .await
            .map_err(|err| err.to_string())?;
        changes.push(json!({"path": line.path, "op": "put", "blob": id, "kind": "regular"}));
    }
    let created = client
        .create_changeset(&ws.owner, &ws.name, ws.tip.as_deref(), message, &changes)
        .await
        .map_err(|err| err.to_string())?;
    let id = created["id"].as_str().unwrap_or("").to_string();
    ws.pending = Some(id.clone());
    ws.save(Path::new("."))?;
    println!("changeset {id}");
    println!("run `scion land` to publish it");
    Ok(())
}

async fn land(strategy: &str) -> Result<(), String> {
    let mut ws = workspace()?;
    let pending = ws
        .pending
        .clone()
        .ok_or("no pending changeset; run scion commit")?;
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    let landed = client
        .land(&ws.owner, &ws.name, &pending, &ws.ref_name, strategy)
        .await
        .map_err(|err| err.to_string())?;
    let tip = landed["landed"].as_str().unwrap_or(&pending).to_string();
    ws.tip = Some(tip.clone());
    ws.pending = None;
    ws.save(Path::new("."))?;
    let files = client
        .list_files(&ws.owner, &ws.name, &ws.ref_name)
        .await
        .map_err(|err| err.to_string())?;
    Workspace::save_index(Path::new("."), &files)?;
    println!("landed {tip}");
    Ok(())
}

async fn update() -> Result<(), String> {
    let mut ws = workspace()?;
    if ws.pending.is_some() {
        return Err("pending changeset; land or discard it before update".into());
    }
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    let sparse = Workspace::load_sparse(Path::new("."))?;
    let info = client
        .get_repo(&ws.owner, &ws.name)
        .await
        .map_err(|err| err.to_string())?;
    let rev = info["default_ref"]
        .as_str()
        .unwrap_or(&ws.ref_name)
        .to_string();
    ws.ref_name = rev.clone();
    let files = match client.list_files(&ws.owner, &ws.name, &rev).await {
        Ok(files) => files,
        Err(err) if err.to_string().contains("no commits") || err.to_string().contains("404") => {
            Vec::new()
        }
        Err(err) => return Err(err.to_string()),
    };
    let index = Workspace::load_index(Path::new("."))?;
    for file in &files {
        if !sparse.matches(&file.path) {
            continue;
        }
        if let Some(previous) = index.iter().find(|entry| entry.path == file.path) {
            if previous.blob == file.blob {
                continue;
            }
            if local_blob(&file.path).as_deref() != Some(previous.blob.as_str()) {
                println!("skipped dirty {}", file.path);
                continue;
            }
        }
        let body = client
            .file(&ws.owner, &ws.name, &rev, &file.path)
            .await
            .map_err(|err| err.to_string())?;
        write_file(&file.path, &body)?;
    }
    ws.tip = files
        .first()
        .map(|_| info["id"].as_str().unwrap_or("").to_string());
    // The repo id is not the tip. Resolve the tip from the log when commits exist.
    if !files.is_empty() {
        if let Ok(log) = client.log(&ws.owner, &ws.name, &rev, 1).await {
            if let Some(id) = log["commits"][0]["id"].as_str() {
                ws.tip = Some(id.to_string());
            }
        }
    }
    ws.save(Path::new("."))?;
    Workspace::save_index(Path::new("."), &files)?;
    println!(
        "updated {} files",
        files.iter().filter(|f| sparse.matches(&f.path)).count()
    );
    Ok(())
}

fn local_blob(path: &str) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(BlobId::of(&bytes).to_hex())
}

fn write_file(path: &str, body: &Value) -> Result<(), String> {
    let bytes = if let Some(text) = body["text"].as_str() {
        text.as_bytes().to_vec()
    } else if let Some(b64) = body["base64"].as_str() {
        base64_decode(b64)?
    } else {
        return Err(format!("no content for {path}"));
    };
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    fs::write(path, bytes).map_err(|err| err.to_string())
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("bad base64".into()),
        }
    }
    let bytes = text
        .as_bytes()
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let mut n = [0u8; 4];
        for (i, byte) in chunk.iter().enumerate() {
            n[i] = val(*byte)?;
        }
        out.push((n[0] << 2) | (n[1] >> 4));
        if chunk.len() > 2 {
            out.push((n[1] << 4) | (n[2] >> 2));
        }
        if chunk.len() > 3 {
            out.push((n[2] << 6) | n[3]);
        }
    }
    Ok(out)
}

async fn log(limit: u32) -> Result<(), String> {
    let ws = workspace()?;
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    let log = client
        .log(&ws.owner, &ws.name, &ws.ref_name, limit)
        .await
        .map_err(|err| err.to_string())?;
    for commit in log["commits"].as_array().unwrap_or(&Vec::new()) {
        let id = commit["id"].as_str().unwrap_or("");
        let short = id.get(..12).unwrap_or(id);
        let subject = commit["message"]
            .as_str()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        println!("{short} {subject}");
    }
    Ok(())
}

async fn blame(path: &str) -> Result<(), String> {
    let ws = workspace()?;
    let auth = load_auth()?;
    let client = client_from_auth(&auth)?;
    let blame = client
        .blame(&ws.owner, &ws.name, &ws.ref_name, path)
        .await
        .map_err(|err| err.to_string())?;
    println!("{}", serde_json::to_string_pretty(&blame).unwrap());
    Ok(())
}

fn sparse(cmd: SparseCmd) -> Result<(), String> {
    match cmd {
        SparseCmd::Show => {
            let spec = fs::read_to_string(Workspace::sparse_path(Path::new(".")))
                .unwrap_or_else(|_| "# full checkout\n".into());
            print!("{spec}");
            Ok(())
        }
        SparseCmd::Set { spec } => {
            SparseSpec::parse(&spec)?;
            let path = Workspace::sparse_path(Path::new("."));
            fs::create_dir_all(path.parent().unwrap()).map_err(|err| err.to_string())?;
            fs::write(path, spec).map_err(|err| err.to_string())?;
            println!("sparse spec written; run scion update");
            Ok(())
        }
    }
}

async fn doctor() -> Result<(), String> {
    let auth = load_auth()?;
    let client = Client::new(&auth.server, auth.insecure).map_err(|err| err.to_string())?;
    let health = client
        .login("doctor-does-not-login", "not-a-password")
        .await;
    match health {
        Err(err) if err.to_string().contains("401") || err.to_string().contains("invalid") => {
            println!("server reachable at {}", auth.server);
            Ok(())
        }
        Ok(_) => Err("unexpected doctor login".into()),
        Err(err) => Err(err.to_string()),
    }
}

async fn token(cmd: TokenCmd) -> Result<(), String> {
    let TokenCmd::Create { name, max_role } = cmd;
    let value = client_token(&name, &max_role).await?;
    println!("{}", value["token"].as_str().unwrap_or(""));
    Ok(())
}

async fn client_token(name: &str, max_role: &str) -> Result<Value, String> {
    // The client crate exposes the session; token creation is a direct call.
    let auth = load_auth()?;
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(auth.insecure)
        .build()
        .map_err(|err| err.to_string())?;
    let response = http
        .post(format!("{}/v1/tokens", auth.server.trim_end_matches('/')))
        .header("x-scion-session", &auth.session_token)
        .header("x-scion-csrf", &auth.csrf)
        .json(&json!({"name": name, "max_role": max_role}))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

async fn server(cmd: ServerCmd) -> Result<(), String> {
    match cmd {
        ServerCmd::Up => server_up().await,
        ServerCmd::Down => compose(&["down"]),
        ServerCmd::Logs => compose(&["logs", "--tail", "200"]),
        ServerCmd::Status => compose(&["ps"]),
        ServerCmd::Restart => compose(&["restart"]),
        ServerCmd::Backup { dest } => {
            compose(&["exec", "-T", "scion-server", "backup", "/tmp/backup"])?;
            let dir = compose_dir()?;
            let status = docker_cmd()
                .args(["compose", "-f"])
                .arg(dir.join("compose.yml"))
                .arg("--project-directory")
                .arg(&dir)
                .args(["cp", "scion-server:/tmp/backup"])
                .arg(&dest)
                .status()
                .map_err(|err| err.to_string())?;
            if status.success() { Ok(()) } else { Err("backup copy failed".into()) }
        }
        ServerCmd::Restore { src: _ } => Err(
            "restore is intentionally not automated: stop the stack, replace the named volume from a backup taken with `scion server backup`, then `scion server up`".into(),
        ),
    }
}

async fn server_up() -> Result<(), String> {
    let dir = compose_dir()?;
    let secrets = dir.join("secrets");
    fs::create_dir_all(&secrets).map_err(|err| err.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&secrets, fs::Permissions::from_mode(0o700));
    }
    let bootstrap = ensure_secret(&secrets.join("bootstrap"))?;
    let _web = ensure_secret(&secrets.join("web"))?;
    ensure_tls(&secrets)?;
    compose(&["up", "-d", "--build", "--wait"])?;
    let port = std::env::var("SCION_SERVER_PORT").unwrap_or_else(|_| "8443".into());
    let client =
        Client::new(format!("https://127.0.0.1:{port}"), true).map_err(|err| err.to_string())?;
    let password_path = secrets.join("initial_admin_password");
    if password_path.exists() {
        println!(
            "stack is up; admin password already written to {}",
            password_path.display()
        );
        return Ok(());
    }
    let password = format!("Init-{}", random_hex(18));
    #[allow(unused_assignments)]
    let mut last = String::from("server did not answer");
    for _ in 0..30 {
        let response = client.login("admin", "not-the-password").await;
        last = format!("{response:?}");
        if last.contains("401") || last.contains("invalid") || last.contains("Status") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|err| err.to_string())?;
    let response = http
        .post(format!("https://127.0.0.1:{port}/v1/setup/first-admin"))
        .bearer_auth(bootstrap.trim())
        .json(&json!({"username": "admin", "password": password}))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status.as_u16() == 409 {
        println!("stack is up; an administrator already exists");
        return Ok(());
    }
    if !status.is_success() {
        return Err(format!(
            "first admin failed: {status} {text} (probe {last})"
        ));
    }
    fs::write(&password_path, &password).map_err(|err| err.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&password_path, fs::Permissions::from_mode(0o400));
    }
    println!("stack is up");
    println!("admin username: admin");
    println!("initial password (printed once): {password}");
    println!("saved to {}", password_path.display());
    println!(
        "change it after logging in at https://localhost:{}",
        std::env::var("SCION_WEB_PORT").unwrap_or_else(|_| "8444".into())
    );
    Ok(())
}

fn ensure_secret(path: &Path) -> Result<String, String> {
    if path.exists() {
        return fs::read_to_string(path).map_err(|err| err.to_string());
    }
    let value = random_hex(32);
    fs::write(path, &value).map_err(|err| err.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o444));
    }
    Ok(value)
}

fn ensure_tls(secrets: &Path) -> Result<(), String> {
    let cert_path = secrets.join("tls.crt");
    let key_path = secrets.join("tls.key");
    if cert_path.exists() && key_path.exists() {
        return Ok(());
    }
    let certified = rcgen::generate_simple_self_signed(vec![
        "localhost".into(),
        "scion-server".into(),
        "127.0.0.1".into(),
    ])
    .map_err(|err| err.to_string())?;
    fs::write(&cert_path, certified.cert.pem()).map_err(|err| err.to_string())?;
    fs::write(&key_path, certified.signing_key.serialize_pem()).map_err(|err| err.to_string())?;
    fs::write(secrets.join("ca.crt"), certified.cert.pem()).map_err(|err| err.to_string())?;
    Ok(())
}

fn random_hex(nbytes: usize) -> String {
    let mut buf = vec![0u8; nbytes];
    let mut file = fs::File::open("/dev/urandom").expect("urandom");
    std::io::Read::read_exact(&mut file, &mut buf).expect("urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn docker_cmd() -> Command {
    let direct = Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if direct.map(|status| status.success()).unwrap_or(false) {
        Command::new("docker")
    } else {
        let mut command = Command::new("sudo");
        command.arg("docker");
        command
    }
}

fn compose(args: &[&str]) -> Result<(), String> {
    let dir = compose_dir()?;
    let status = docker_cmd()
        .arg("compose")
        .arg("-f")
        .arg(dir.join("compose.yml"))
        .arg("--project-directory")
        .arg(&dir)
        .args(args)
        .status()
        .map_err(|err| format!("docker compose: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("docker compose exited {status}"))
    }
}

fn compose_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("SCION_COMPOSE") {
        return Ok(PathBuf::from(path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf());
    }
    let mut dir = std::env::current_dir().map_err(|err| err.to_string())?;
    loop {
        if dir.join("docker/compose.yml").is_file() {
            return Ok(dir.join("docker"));
        }
        if !dir.pop() {
            break;
        }
    }
    Err("could not find docker/compose.yml; set SCION_COMPOSE".into())
}
