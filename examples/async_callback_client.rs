// try this example with
// `cargo run --example async_callback_client`

use fast_websocket_client::WebSocketBuilder;

#[tokio::main]
async fn main() -> Result<(), fast_websocket_client::WebSocketClientError> {
    let ws = WebSocketBuilder::new()
        .on_open(|| {
            println!("[OPEN] WebSocket connection opened.");
        })
        .on_close(|| {
            println!("[CLOSE] WebSocket connection closed.");
        })
        .on_error(|e| {
            println!("[ERROR] {}", e);
        })
        .on_message(|message| {
            println!("[MESSAGE] {}", message);
        })
        .connect("wss://ws-api.binance.com:443/ws-api/v3")
        .await?;

    println!("await_shutdown");
    match ws.join().await {
        Ok(_) => println!("ws end"),
        Err(e) => println!("ws error: {:?}", e),
    }

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
