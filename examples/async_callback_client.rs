// try this example with
// `cargo run --example wss_client`

use fast_websocket_client::{ClientCommand, WebSocket, WebSocketBuilder};
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn test_ws() -> Result<(), fast_websocket_client::WebSocketClientError> {
    let mut ws_builder = WebSocketBuilder::new();
    ws_builder = ws_builder.on_open(|tx| async move {
        println!("[OPEN] WebSocket connection opened.");
        // let _ = tx.send(ClientCommand::SendMessage("Hello, world!".to_string()));
        sleep(Duration::from_secs(2)).await;
        todo!("test panic");
    });
    ws_builder = ws_builder.on_close(|_| async move {
        println!("[CLOSE] WebSocket connection closed.");
    });
    ws_builder = ws_builder.on_error(|e| async move {
        println!("[ERROR] {}", e);
    });
    ws_builder = ws_builder.on_message(|message| async move {
        println!("[MESSAGE] {}", message);
    });
    let ws = ws_builder
        .connect("wss://ws-api.binance.com:443/ws-api/v3")
        .await?;

    println!("await_shutdown");
    // ws.await_shutdown().await;
    // println!("await_shutdown done");
    match ws.join().await {
        Ok(_) => println!("ws end"),
        Err(e) => println!("ws error: {:?}", e),
    }
    // loop {
    //     println!("loop");
    //     sleep(Duration::from_secs(1)).await;
    // }

    // sleep(Duration::from_secs(1)).await;
    // for i in 1..5 {
    //     let message = format!("#{}", i);
    //     if let Err(e) = ws.send(&message).await {
    //         eprintln!("[ERROR] Send error: {:?}", e);
    //         break;
    //     }
    //     println!("[SEND] {}", message);
    //     sleep(Duration::from_secs(5)).await;
    // }

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
