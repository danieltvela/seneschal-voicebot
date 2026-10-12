use clap::Parser;
use db_inspect::config::DbConfig;
use db_inspect::db::AppState;
use db_inspect::routes::create_router;
use std::sync::Arc;

/// Seneschal database inspector — standalone viewer for Seneschal's SQLite database.
#[derive(Parser, Debug)]
#[command(name = "db-inspect")]
struct Args {
    /// Path to the Seneschal SQLite database
    #[arg(long)]
    db: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = DbConfig::from_args(args.db);

    let app_state = Arc::new(AppState::new(&cfg.db_path).await?);
    let app = create_router(app_state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_string()).await?;
    println!("Listening on {} (DB: {})", cfg.bind_string(), cfg.db_path);

    axum::serve(listener, app).await?;

    Ok(())
}
