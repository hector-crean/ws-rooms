use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use std::sync::Arc;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpStream,
    signal::ctrl_c,       // Add this import for handling Ctrl+C
    sync::{Mutex, watch}, // Use tokio's Mutex
    time::Instant,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};

// Type aliases for clarity
type WsWriter = Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>;
type WsReader = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

// Send pings slightly more often than the server expects to hear from us
const HEARTBEAT_INTERVAL_SECONDS: u64 = 30;
// Allow ample time for server ping + our pong + network latency
const HEARTBEAT_TIMEOUT_SECONDS: u64 = 60; // Should match or be slightly > server timeout

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let room_id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "default_room".to_string());

    let client_id = uuid::Uuid::new_v4();
    let url = format!("ws://0.0.0.0:8080/ws/room/{}", room_id);
    tracing::info!(%client_id, %room_id, "Connecting to {}", url);

    // --- Connection ---
    let (ws_stream, response) = connect_async(url)
        .await
        .map_err(|e| format!("Failed to connect: {}", e))?;
    tracing::info!(%client_id, %room_id, "Connected successfully!");
    tracing::debug!(%client_id, %room_id, "Server response: {:?}", response);

    let (write, read) = ws_stream.split();
    let writer: WsWriter = Arc::new(Mutex::new(write));

    // --- Shutdown Signal ---
    // Use a watch channel to notify tasks to shut down gracefully.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // --- Last Pong Tracking ---
    // Track the time the last Pong was received. Initialize with Now to prevent
    // immediate timeout before the first ping/pong cycle completes.
    let last_pong_received = Arc::new(Mutex::new(Some(Instant::now())));

    // --- Spawn Tasks ---
    let heartbeat_handle = tokio::spawn(heartbeat_task(
        writer.clone(),
        last_pong_received.clone(),
        shutdown_rx.clone(),
        shutdown_tx.clone(),
        client_id,
        room_id.clone(),
    ));

    let sender_handle = tokio::spawn(sender_task(
        writer.clone(),
        shutdown_rx.clone(),
        shutdown_tx.clone(),
        client_id,
        room_id.clone(),
    ));

    let receiver_handle = tokio::spawn(receiver_task(
        read, // Pass reader ownership
        last_pong_received,
        shutdown_rx.clone(),
        shutdown_tx.clone(), // Pass sender to signal shutdown on close/error
        client_id,
        room_id.clone(),
    ));

    // --- Spawn Ctrl+C handler ---
    let shutdown_tx_ctrlc = shutdown_tx.clone();
    let client_id_ctrlc = client_id;
    let room_id_ctrlc = room_id.clone();
    let ctrl_c_handle = tokio::spawn(async move {
        if let Err(e) = ctrl_c().await {
            tracing::error!(%client_id_ctrlc, %room_id_ctrlc, "Failed to listen for Ctrl+C: {}", e);
            return;
        }
        tracing::info!(%client_id_ctrlc, %room_id_ctrlc, "Ctrl+C received, initiating graceful shutdown...");
        let _ = shutdown_tx_ctrlc.send(true);
    });

    // --- Wait for tasks or shutdown signal ---
    // Wait for any task to finish naturally or signal shutdown
    let mut shutdown_rx_main = shutdown_rx.clone(); // Clone for main select
    tokio::select! {
        result = heartbeat_handle => tracing::info!("Heartbeat task finished: {:?}", result),
        result = sender_handle => tracing::info!("Sender task finished: {:?}", result),
        result = receiver_handle => tracing::info!("Receiver task finished: {:?}", result),
        result = ctrl_c_handle => tracing::info!("Ctrl+C handler finished: {:?}", result),
        // Also listen for the shutdown signal directly in main
        _ = shutdown_rx_main.changed() => {
            if *shutdown_rx_main.borrow() {
                tracing::info!("Shutdown signal received in main, initiating termination.");
            }
         }
    }

    // --- Cleanup ---
    // Ensure shutdown is signalled if not already (e.g., if main select exits before signal)
    let _ = shutdown_tx.send(true);

    tracing::info!("Attempting graceful shutdown of writer...");
    // Give tasks a brief moment to react to the shutdown signal
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Try to close the WebSocket connection gracefully
    match writer.try_lock() {
        Ok(mut writer_locked) => {
            if let Err(e) = writer_locked.close().await {
                tracing::warn!("Error closing WebSocket writer: {}", e);
            } else {
                tracing::info!("WebSocket writer closed gracefully.");
            }
        }
        Err(e) => tracing::warn!("Could not acquire writer lock for closing: {}", e),
    }

    // Optionally, wait for tasks to fully complete after shutdown signal
    // (though they should exit quickly due to the select loops)
    // let _ = tokio::join!(heartbeat_handle, sender_handle, receiver_handle); // This might hang if a task doesn't exit

    tracing::info!("Client exiting.");
    Ok(())
}

async fn heartbeat_task(
    writer: WsWriter,
    last_pong_received: Arc<Mutex<Option<Instant>>>,
    mut shutdown_rx: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
    client_id: uuid::Uuid,
    room_id: String,
) {
    let heartbeat_interval = Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS);
    // Timeout specific to waiting for Pong *after client sends Ping*
    let pong_timeout_duration = Duration::from_secs(HEARTBEAT_TIMEOUT_SECONDS);
    let mut interval = tokio::time::interval(heartbeat_interval);

    loop {
        tokio::select! {
            // Wait for the next tick or shutdown signal
            _ = interval.tick() => {
                // Check if we received a pong recently enough *in response to our pings*
                // This logic assumes the pong we receive corresponds roughly to the last ping we sent.
                // More robust checks might involve ping payloads, but this is common.
                let pong_deadline = Instant::now() - pong_timeout_duration;
                let last_pong = *last_pong_received.lock().await;

                if let Some(last_pong_time) = last_pong {
                    // Note: This check is slightly simplified. It ensures *a* pong was received
                    // within the timeout period, not necessarily the pong for *this specific* ping.
                    // For most keep-alive purposes, this is sufficient.
                    if last_pong_time < pong_deadline {
                       tracing::warn!(%client_id, %room_id, "Client Heartbeat timeout: No Pong received from server recently. Assuming connection lost.");
                        let _ = shutdown_tx.send(true); // Signal shutdown
                        break; // Exit loop
                    }
                } else {
                     // If no pong ever received after connection start + first interval
                     let elapsed_since_start = Instant::now().duration_since(last_pong_received.lock().await.unwrap()); // unwrap safe due to init
                     if elapsed_since_start > pong_timeout_duration {
                        tracing::warn!(%client_id, %room_id, "Client Heartbeat check: No Pong ever received from server. Assuming connection lost.");
                        let _ = shutdown_tx.send(true); // Signal shutdown
                        break; // Exit loop
                     }
                     // else: Still within initial grace period, wait longer
                }

                // Send Ping to Server
                tracing::debug!(%client_id, %room_id, "Client sending heartbeat Ping");
                let mut writer_locked = writer.lock().await;
                 // Send empty ping payload
                if let Err(e) = writer_locked.send(Message::Ping(vec![].into())).await {
                    tracing::warn!(%client_id, %room_id, "Client failed to send Ping: {}. Assuming connection lost.", e);
                    let _ = shutdown_tx.send(true); // Signal shutdown
                    break; // Exit loop
                }
                 drop(writer_locked);
            }
            // Check if shutdown was requested
             _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!(%client_id, %room_id, "Client Heartbeat task received shutdown signal.");
                    break; // Exit loop
                }
            }
        }
    }
    tracing::info!(%client_id, %room_id, "Client Heartbeat task exiting.");
}

// --- Sender Task (Stdin) ---
async fn sender_task(
    writer: WsWriter,
    mut shutdown_rx: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
    client_id: uuid::Uuid,
    room_id: String,
) {
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            // Wait for stdin input or shutdown signal
             result = stdin.next_line() => {
                 match result {
                    Ok(Some(line)) => {
                        if line.is_empty() { continue; } // Skip empty lines
                        tracing::debug!(%client_id, %room_id, "Sending message: {}", line);
                         let mut writer_locked = writer.lock().await;
                         if let Err(e) = writer_locked.send(Message::Text(line.into())).await {
                            tracing::error!(%client_id, %room_id, "Failed to send message: {}", e);
                            let _ = shutdown_tx.send(true); // Signal shutdown
                            break; // Exit loop
                        }
                         // Drop the lock promptly
                         drop(writer_locked);
                    }
                    Ok(None) => {
                        // Stdin closed (e.g., Ctrl+D)
                        tracing::info!(%client_id, %room_id, "Stdin closed.");
                        // Optionally signal shutdown here if desired, or just let the task end.
                        // let _ = shutdown_tx.send(true);
                        break; // Exit loop
                    }
                    Err(e) => {
                        tracing::error!(%client_id, %room_id, "Error reading from stdin: {}", e);
                        let _ = shutdown_tx.send(true); // Signal shutdown on stdin error
                        break; // Exit loop
                    }
                 }
             }
             // Check if shutdown was requested
             _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!(%client_id, %room_id, "Sender task received shutdown signal.");
                    break; // Exit loop
                }
            }
        }
    }
    tracing::info!(%client_id, %room_id, "Sender task exiting.");
}

// --- Receiver Task ---
async fn receiver_task(
    mut reader: WsReader,
    last_pong_received: Arc<Mutex<Option<Instant>>>,
    mut shutdown_rx: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
    client_id: uuid::Uuid,
    room_id: String,
) {
    loop {
        tokio::select! {
            // Wait for a message or shutdown signal
            message_result = reader.next() => {
                match message_result {
                    Some(Ok(msg)) => {
                        match msg {
                            Message::Text(text) => {
                                println!("[Client {}] Received: {}", client_id, text);
                            }
                            Message::Binary(bin) => {
                                tracing::info!(%client_id, %room_id, "Received binary data: {} bytes", bin.len());
                            }
                            Message::Ping(payload) => {
                                tracing::debug!(%client_id, %room_id, "Received Ping: {:?}", payload);
                                // tokio-tungstenite automatically handles sending Pongs for Pings.
                            }
                            Message::Pong(payload) => {
                                tracing::debug!(%client_id, %room_id, "Received Pong: {:?}", payload);
                                // Update the last pong time
                                *last_pong_received.lock().await = Some(Instant::now());
                            }
                            Message::Close(frame) => {
                                tracing::info!(%client_id, %room_id, "Received Close frame: {:?}", frame);
                                let _ = shutdown_tx.send(true); // Signal shutdown
                                break; // Exit loop
                            }
                            Message::Frame(_) => {
                                // Low-level frame, usually ignore unless debugging internals
                                tracing::trace!(%client_id, %room_id, "Received raw frame");
                            }
                        }
                    }
                    Some(Err(e)) => {
                        // Error reading from the WebSocket stream
                        tracing::error!(%client_id, %room_id, "Error receiving message: {}", e);
                        let _ = shutdown_tx.send(true); // Signal shutdown
                        break; // Exit loop
                    }
                    None => {
                        // WebSocket stream closed gracefully from the other side
                        // without sending a Close frame (less common, but possible)
                        tracing::info!(%client_id, %room_id, "WebSocket stream closed by peer (no Close frame).");
                        let _ = shutdown_tx.send(true); // Signal shutdown
                        break; // Exit loop
                    }
                }
            }
            // Check if shutdown was requested
             _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::debug!(%client_id, %room_id, "Receiver task received shutdown signal.");
                    break; // Exit loop
                }
            }
        }
    }
    tracing::info!(%client_id, %room_id, "Receiver task exiting.");
}
