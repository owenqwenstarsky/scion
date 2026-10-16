fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();
    let outcome = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(scion_server::run());
    if let Err(err) = outcome {
        eprintln!("{err}");
        std::process::exit(scion_server::code(&err));
    }
}
