//! Headless Covalence IO server.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use covalence_server::{App, Config, DEFAULT_UI_DIR, PluginSpec, router, with_ui};

#[derive(Parser)]
#[command(version, about = "Covalence IO headless server")]
struct Args {
    /// Address for the HTTP API.
    #[arg(long, default_value = "127.0.0.1:7680")]
    listen: SocketAddr,
    /// Directory for the project database.
    #[arg(long, default_value = "data")]
    data: PathBuf,
    /// Repository (or install) root: holds the plugin host and plugins.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// A plugin to load, as <id>@<version>=<bundle>. Repeatable. Defaults to
    /// the first-party plugins.
    #[arg(long = "plugin")]
    plugins: Vec<PluginSpec>,
    /// The deno executable.
    #[arg(long, default_value = "deno")]
    deno: PathBuf,
    /// The built web UI to serve at `/`. Defaults to packages/ui/dist under
    /// the root, when it has been built.
    #[arg(long)]
    ui: Option<PathBuf>,
    /// Serve the API only.
    #[arg(long, conflicts_with = "ui")]
    no_ui: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    // An explicit --ui must exist; the default is skipped when it hasn't been
    // built. Checked before the plugin host starts.
    let ui = match (&args.ui, args.no_ui) {
        (_, true) => None,
        (Some(dir), false) if !dir.join("index.html").is_file() => {
            return Err(format!(
                "{} has no index.html: build the UI with `pnpm --filter ui build`",
                dir.display()
            )
            .into());
        }
        (Some(dir), false) => Some(dir.clone()),
        (None, false) => {
            let dir = args.root.join(DEFAULT_UI_DIR);
            if dir.join("index.html").is_file() {
                Some(dir)
            } else {
                tracing::info!(
                    "no UI at {}: serving the API only (build it with `pnpm --filter ui build`)",
                    dir.display()
                );
                None
            }
        }
    };
    let plugins = if args.plugins.is_empty() {
        Config::default_plugins()
    } else {
        args.plugins
    };
    let app = App::start(Config {
        data_dir: args.data,
        root: args.root,
        plugins,
        deno: args.deno,
    })?;

    let mut routes = router(app.clone());
    if let Some(dir) = &ui {
        routes = with_ui(routes, dir).map_err(|e| e.to_string())?;
        tracing::info!("serving the UI from {}", dir.display());
    }

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(
        "covalence-server {} (protocol {}) listening on http://{}",
        env!("CARGO_PKG_VERSION"),
        covalence_core::PROTOCOL_VERSION,
        listener.local_addr()?
    );
    axum::serve(listener, routes)
        .with_graceful_shutdown(async {
            stop_signal().await;
            tracing::info!("shutting down");
        })
        .await?;
    app.shutdown().await;
    Ok(())
}

/// Ctrl-C, or SIGTERM on Unix (service managers send it).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("can listen for SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
