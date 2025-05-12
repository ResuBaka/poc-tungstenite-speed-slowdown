//! Based on tokio-tungstenite example websocket client, but with multiple
//! concurrent websocket clients in one package
//!
//! This will connect to a server specified in the SERVER with N_CLIENTS
//! concurrent connections, and then flood some test messages over websocket.
//! This will also print whatever it gets into stdout.
//!
//! Note that this is not currently optimized for performance, especially around
//! stdout mutex management. Rather it's intended to show an example of working with axum's
//! websocket server and how the client-side and server-side code can be quite similar.
//!

use futures_util::stream::FuturesUnordered;
use futures_util::{SinkExt, StreamExt};
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

// we will use tungstenite for websocket client impl (same library as what axum is using)
use tokio_tungstenite::{connect_async_with_config, tungstenite::protocol::{Message}};
use tokio_tungstenite::tungstenite::protocol::frame::Payload;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const N_CLIENTS: usize = 1; //set to desired number
const SERVER: &str = "ws://127.0.0.1:3000/ws";

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=info,tower_http=info", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let start_time = Instant::now();
    //spawn several clients that will concurrently talk to the server
    let mut clients = (0..N_CLIENTS)
        .map(|cli| tokio::spawn(spawn_client(cli)))
        .collect::<FuturesUnordered<_>>();

    //wait for all our clients to exit
    while clients.next().await.is_some() {}

    let end_time = Instant::now();

    //total time should be the same no matter how many clients we spawn
    tracing::info!(
        "Total time taken {:#?} with {N_CLIENTS} concurrent clients, should be about 37 to 40 seconds.",
        end_time - start_time
    );
}

//creates a client. quietly exits on failure.
async fn spawn_client(who: usize) {
    let websocket_config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let ws_stream = match connect_async_with_config(SERVER, Some(websocket_config), false).await {
        Ok((stream, response)) => {
            tracing::info!("Handshake for client {who} has been completed");
            // This will be the HTTP response, same as with server this is the last moment we
            // can still access HTTP stuff.
            tracing::info!("Server response was {response:?}");
            stream
        }
        Err(e) => {
            tracing::error!("WebSocket handshake for client {who} failed with {e}!");
            return;
        }
    };

    let (mut sender, mut receiver) = ws_stream.split();

    //we can ping the server for start
    sender
        .send(Message::Ping(Payload::from(
            b"Hello, Server!".to_vec(),
        )))
        .await
        .expect("Can not send!");

    //spawn an async sender to push some more messages into the server
    // let mut send_task = tokio::spawn(async move {
    //     tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        // for i in 1..30 {
        //     // In any websocket error, break loop.
        //     if sender
        //         .send(Message::Text(format!("Message number {i}...").into()))
        //         .await
        //         .is_err()
        //     {
        //         //just as with server, if send fails there is nothing we can do but exit.
        //         return;
        //     }
        //
        //     tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        // }

        // // When we are done we may want our client to close connection cleanly.
        // println!("Sending close to {who}...");
        // if let Err(e) = sender
        //     .send(Message::Close(Some(CloseFrame {
        //         code: CloseCode::Normal,
        //         reason: Utf8Bytes::from_static("Goodbye"),
        //     })))
        //     .await
        // {
        //     println!("Could not send Close due to {e:?}, probably it is ok?");
        // };
    // });

    let mut recv_task = tokio::spawn(async move {
        let mut start = Instant::now();
        while let Some(msg) = receiver.next().await {
            // print message and break if instructed to do so

            if let Ok(msg) = msg {
                if process_message(msg, who, &mut start).is_break() {
                    break;
                }
            } else if let Err(e) = msg {
                tracing::error!("Error while receiving message {e}!");
                break;
            }
        }
    });

    //wait for either task to finish and kill the other task
    tokio::select! {
        // _ = (&mut send_task) => {
        //     recv_task.abort();
        // },
        _ = (&mut recv_task) => {
            // send_task.abort();
        }
    }
}

/// Function to handle messages we get (with a slight twist that Frame variant is visible
/// since we are working with the underlying tungstenite library directly without axum here).
fn process_message(msg: Message, who: usize, last_text_message: &mut Instant) -> ControlFlow<(), ()> {
    match msg {
        Message::Text(_t) => {
            // tracing::info!(">>> {who} got str: {t:?}");
            *last_text_message = Instant::now();
        }
        Message::Binary(d) => {
            let elapsed = last_text_message.elapsed();

            println!(">>> {} took ~{:#?} with speed ~{}", human_readable_bytes(d.len()), elapsed, human_readable_speed(d.len(), elapsed));

        }
        Message::Close(c) => {
            if let Some(cf) = c {
                tracing::info!(
                    ">>> {} got close with code {} and reason `{}`",
                    who, cf.code, cf.reason
                );
            } else {
                tracing::error!(">>> {who} somehow got close message without CloseFrame");
            }
            return ControlFlow::Break(());
        }

        Message::Pong(v) => {
            tracing::info!(">>> {who} got pong with {v:?}");
        }
        // Just as with axum server, the underlying tungstenite websocket library
        // will handle Ping for you automagically by replying with Pong and copying the
        // v according to spec. But if you need the contents of the pings you can see them here.
        Message::Ping(v) => {
            tracing::info!(">>> {who} got ping with {v:?}");
        }

        Message::Frame(_) => {
            unreachable!("This is never supposed to happen")
        }
    }
    ControlFlow::Continue(())
}

fn human_readable_bytes(num_bytes: usize) -> String {
    if num_bytes == 0 {
        return "0 Bytes".to_string();
    }

    let units = ["Bytes", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    let mut size = num_bytes as f64;
    let mut i = 0;

    while size >= 1024.0 && i < units.len() - 1 {
        size /= 1024.0;
        i += 1;
    }

    format!("{:.1} {}", size, units[i])
}

fn human_readable_speed(bytes: usize, elapsed_time: Duration) -> String {
    let elapsed_seconds = elapsed_time.as_secs_f64();

    if elapsed_seconds == 0.0 {
        return "0 Bytes/s".to_string();
    }

    let bytes_per_second = bytes as f64 / elapsed_seconds;

    let units = ["Bytes", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
    let mut size = bytes_per_second;
    let mut i = 0;

    while size >= 1024.0 && i < units.len() - 1 {
        size /= 1024.0;
        i += 1;
    }

    format!("{:.1} {}/s", size, units[i])
}