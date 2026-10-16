use rustls::ServerConfig;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

pub fn load_server_config(cert_path: &str, key_path: &str) -> Result<Arc<ServerConfig>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut certs_reader =
        BufReader::new(File::open(cert_path).map_err(|err| format!("{cert_path}: {err}"))?);
    let certs = rustls_pemfile::certs(&mut certs_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    let mut key_reader =
        BufReader::new(File::open(key_path).map_err(|err| format!("{key_path}: {err}"))?);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("{key_path}: no private key"))?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|err| err.to_string())?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
