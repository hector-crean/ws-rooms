use ws_rooms::server::App;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG) // Use DEBUG for heartbeat traces
        .init();

    let app = App::new();
    app.run(8080).await?;

    Ok(())
}
