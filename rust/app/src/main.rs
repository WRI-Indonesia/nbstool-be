use std::net::SocketAddr;

use axum::serve::ListenerExt;

/// The folder the server runs in: `.env`, `assets/`, `generated-file/` and the other relative
/// paths are read from it, as the original reads them from its working directory.
/// `NBSTOOL_HOME`, else the current directory when it holds `assets/`, else the executable's
/// directory when that does, else the current directory.
fn enter_home() {
    use std::path::PathBuf;
    let has_assets = |d: &PathBuf| d.join("assets").is_dir();
    let home = match std::env::var_os("NBSTOOL_HOME") {
        Some(home) => PathBuf::from(home),
        None => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(PathBuf::from));
            match exe_dir {
                Some(dir) if !has_assets(&cwd) && has_assets(&dir) => dir,
                _ => cwd,
            }
        }
    };
    if let Err(e) = std::env::set_current_dir(&home) {
        eprintln!("NBSTOOL_HOME {}: {e}", home.display());
        std::process::exit(1);
    }
    if !has_assets(&home) {
        eprintln!(
            "warning: {} has no assets/ folder (set NBSTOOL_HOME)",
            home.display()
        );
    }
}

#[tokio::main]
async fn main() {
    enter_home();
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))
        .await
        .expect("binding the port");
    let listener = listener.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    // the peer address reaches handlers (Flask-Login's session identifier hashes it)
    let app = nbstool_be::routes::router();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("serving");
}

/// Ctrl-C or SIGTERM: stop taking requests and finish the open ones.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("a SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
