//! Process entry. The container check runs before the data directory is opened.

use crate::app::App;
use crate::guard::{require_container, StartupError, SystemEnv, TlsMode};
use crate::http::router;
use crate::tls;
use axum::routing::get;
use axum::Router;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::ServerConfig;
use scion_iam::PasswordParams;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

#[derive(Debug)]
pub enum Exit {
    Container(StartupError),
    Fail(String),
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exit::Container(err) => write!(f, "{err}"),
            Exit::Fail(err) => write!(f, "scion-server: {err}"),
        }
    }
}

pub fn code(err: &Exit) -> i32 {
    match err {
        Exit::Container(_) => 78,
        Exit::Fail(_) => 1,
    }
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn read_token_file(path: &str) -> Result<String, Exit> {
    let mut bytes = std::fs::read(path).map_err(|err| Exit::Fail(format!("{path}: {err}")))?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|_| Exit::Fail(format!("{path}: token is not utf-8")))
}

pub async fn run() -> Result<(), Exit> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let cmd = args.first().map(String::as_str).unwrap_or("serve");
    require_container(&SystemEnv).map_err(Exit::Container)?;
    match cmd {
        "healthcheck" => healthcheck().await,
        "backup" => backup(args.get(1).map(String::as_str)),
        "serve" => serve().await,
        other => Err(Exit::Fail(format!("unknown command `{other}`"))),
    }
}

async fn healthcheck() -> Result<(), Exit> {
    let addr = var("SCION_ADMIN_LISTEN").unwrap_or_else(|| "127.0.0.1:9100".into());
    let mut stream = tokio::net::TcpStream::connect(&addr)
        .await
        .map_err(|err| Exit::Fail(format!("healthcheck: {err}")))?;
    let request = format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .map_err(|err| Exit::Fail(err.to_string()))?;
    let mut buf = vec![0u8; 256];
    let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
        .await
        .map_err(|err| Exit::Fail(err.to_string()))?;
    let text = String::from_utf8_lossy(&buf[..n]);
    if text.contains("200") {
        Ok(())
    } else {
        Err(Exit::Fail(format!("healthcheck failed: {text}")))
    }
}

fn backup(dest: Option<&str>) -> Result<(), Exit> {
    let dest = dest.ok_or_else(|| Exit::Fail("usage: scion-server backup <directory>".into()))?;
    let data = var("SCION_DATA").unwrap_or_else(|| "/var/lib/scion".into());
    let app = open_for_maintenance(&data)?;
    std::fs::create_dir_all(dest).map_err(|err| Exit::Fail(err.to_string()))?;
    let meta = PathBuf::from(dest).join("meta.db");
    let small = PathBuf::from(dest).join("small-blobs.db");
    app.store
        .backup_sqlite(&meta, &small)
        .map_err(|err| Exit::Fail(err.to_string()))?;
    println!("backed up to {dest}");
    Ok(())
}

fn open_for_maintenance(data: &str) -> Result<Arc<App>, Exit> {
    App::open(data, None, None, PasswordParams::default(), true)
        .map_err(|err| Exit::Fail(err.to_string()))
}

async fn serve() -> Result<(), Exit> {
    let data = var("SCION_DATA").unwrap_or_else(|| "/var/lib/scion".into());
    let listen = parse_public_listen()?;
    let admin = parse_admin_listen()?;
    let tls_mode = match var("SCION_TLS_MODE").as_deref() {
        Some("disabled") => TlsMode::Disabled,
        _ => TlsMode::Required,
    };
    let allow_plain = var("SCION_ALLOW_INSECURE_PLAINTEXT").as_deref() == Some("1");
    let bootstrap = match var("SCION_BOOTSTRAP_TOKEN_FILE") {
        Some(path) => Some(read_token_file(&path)?),
        None => None,
    };
    let web = match var("SCION_WEB_TOKEN_FILE") {
        Some(path) => Some(read_token_file(&path)?),
        None => None,
    };
    let argon = if var("SCION_ARGON_FAST").as_deref() == Some("1") {
        PasswordParams::fast_for_tests()
    } else {
        PasswordParams::default()
    };
    let app = App::open(
        &data,
        bootstrap.as_deref(),
        web.as_deref(),
        argon,
        tls_mode == TlsMode::Disabled,
    )
    .map_err(|err| Exit::Fail(err.to_string()))?;
    app.credential_gate(
        bootstrap.as_ref().map(|t| t.as_bytes()),
        tls_mode,
        allow_plain,
    )
    .map_err(|err| match err {
        StartupError::NotInContainer => Exit::Container(err),
        other => Exit::Fail(other.to_string()),
    })?;
    let tls = if tls_mode == TlsMode::Required {
        let cert = var("SCION_TLS_CERT_FILE")
            .ok_or_else(|| Exit::Fail("SCION_TLS_CERT_FILE is required".into()))?;
        let key = var("SCION_TLS_KEY_FILE")
            .ok_or_else(|| Exit::Fail("SCION_TLS_KEY_FILE is required".into()))?;
        Some(tls::load_server_config(&cert, &key).map_err(Exit::Fail)?)
    } else {
        tracing::warn!("plaintext listener; SCION_ALLOW_INSECURE_PLAINTEXT=1");
        None
    };
    std::fs::create_dir_all(&data).map_err(|err| Exit::Fail(err.to_string()))?;
    listen_all(app, listen, admin, tls)
        .await
        .map_err(|err| Exit::Fail(err.to_string()))
}

fn parse_public_listen() -> Result<SocketAddr, Exit> {
    let raw = var("SCION_LISTEN").unwrap_or_else(|| "0.0.0.0:8443".into());
    let addr: SocketAddr = raw
        .parse()
        .map_err(|err| Exit::Fail(format!("SCION_LISTEN: {err}")))?;
    if !addr.ip().is_unspecified() && var("SCION_ALLOW_LOOPBACK").as_deref() != Some("1") {
        return Err(Exit::Fail(
            "SCION_LISTEN must publish on 0.0.0.0 (or ::). Set SCION_ALLOW_LOOPBACK=1 to override."
                .into(),
        ));
    }
    Ok(addr)
}

fn parse_admin_listen() -> Result<SocketAddr, Exit> {
    let raw = var("SCION_ADMIN_LISTEN").unwrap_or_else(|| "127.0.0.1:9100".into());
    raw.parse()
        .map_err(|err| Exit::Fail(format!("SCION_ADMIN_LISTEN: {err}")))
}

async fn listen_all(
    app: Arc<App>,
    listen: SocketAddr,
    admin: SocketAddr,
    tls: Option<Arc<ServerConfig>>,
) -> std::io::Result<()> {
    let public = router(app);
    let admin_router = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/metrics",
            get(|| async { "# HELP scion_up 1\nscion_up 1\n" }),
        );
    let admin_listener = TcpListener::bind(admin).await?;
    let public_listener = TcpListener::bind(listen).await?;
    tracing::info!(%listen, %admin, tls = tls.is_some(), "scion-server listening");
    let admin_task = tokio::spawn(async move { axum::serve(admin_listener, admin_router).await });
    let public_task = tokio::spawn(async move {
        if let Some(config) = tls {
            serve_tls(public_listener, public, config).await
        } else {
            axum::serve(public_listener, public).await
        }
    });
    tokio::select! {
        result = admin_task => result.map_err(std::io::Error::other)?,
        result = public_task => result.map_err(std::io::Error::other)?,
    }
}

async fn serve_tls(
    listener: TcpListener,
    app: Router,
    config: Arc<ServerConfig>,
) -> std::io::Result<()> {
    let acceptor = TlsAcceptor::from(config);
    loop {
        let (tcp, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            let io = TokioIo::new(tls);
            let service = service_fn(move |request| {
                let app = app.clone();
                async move { app.oneshot(request).await }
            });
            let builder =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
            if let Err(err) = builder.serve_connection(io, service).await {
                tracing::debug!(%err, "connection closed");
            }
        });
    }
}
