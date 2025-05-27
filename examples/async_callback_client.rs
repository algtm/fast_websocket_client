// try this example with
// `cargo run --example wss_client`

use fast_websocket_client::{ClientCommand, WebSocket};
use tokio::time::{Duration, sleep};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), fast_websocket_client::WebSocketClientError> {
    let ws = WebSocket::new("wss://echo.websocket.org").await?;

    ws.on_close(|_| async move {
        println!("[CLOSE] WebSocket connection closed.");
    })
    .await;
    ws.on_message(|message| async move {
        println!("[MESSAGE] {}", message);
    })
    .await;

    ws.on_open(|tx| async move {
        println!("[OPEN] WebSocket connection opened.");
        let _ = tx.send(ClientCommand::SendMessage("Hello, world!".to_string()));
    })
    .await;

    sleep(Duration::from_secs(1)).await;
    for i in 1..5 {
        let message = format!("#{}", i);
        if let Err(e) = ws.send(&message).await {
            eprintln!("[ERROR] Send error: {:?}", e);
            break;
        }
        println!("[SEND] {}", message);
        sleep(Duration::from_secs(5)).await;
    }

    ws.close().await;
    ws.await_shutdown().await;
    Ok(())
}

/* JavaScript equivalent
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>WebSocket Client</title>
</head>
<body>
  <script>
    function sleep(ms) {
      return new Promise(resolve => setTimeout(resolve, ms));
    }

    async function main() {
      const ws = new WebSocket("wss://echo.websocket.org");

      ws.onclose = () => {
        console.log("[CLOSE] WebSocket connection closed.");
      };
      ws.onmessage = (event) => {
        console.log("[MESSAGE]", event.data);
      };

      await sleep(1000);
      for (let i = 1; i < 5; i++) {
        const message = `#${i}`;
        try {
          ws.send(message);
          console.log("[SEND]", message);
        } catch (err) {
          console.error("[ERROR] Send error:", err);
          break;
        }
        await sleep(5000);
      }

      ws.close();
    }

    main();
  </script>
</body>
</html>
*/
