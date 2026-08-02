use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc as std_mpsc,
    thread,
    time::Duration,
};

use fast_websocket_client::{ClientConfig, WebSocketBuilder};
use tokio::sync::mpsc;

const EXPECTED: &str = r#"{"topic":"order","seq":1}"#;
const INTERRUPT: &str = r#"{"op":"interrupt"}"#;

fn read_raw_client_opcode(stream: &mut TcpStream) -> u8 {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).unwrap();

    let masked = header[1] & 0x80 != 0;
    assert!(masked, "client WebSocket frames must be masked");

    let payload_len = match header[1] & 0x7f {
        126 => {
            let mut length = [0_u8; 2];
            stream.read_exact(&mut length).unwrap();
            u16::from_be_bytes(length) as usize
        }
        127 => {
            let mut length = [0_u8; 8];
            stream.read_exact(&mut length).unwrap();
            usize::try_from(u64::from_be_bytes(length)).unwrap()
        }
        length => length as usize,
    };

    let mut mask = [0_u8; 4];
    stream.read_exact(&mut mask).unwrap();
    let mut payload = vec![0_u8; payload_len];
    stream.read_exact(&mut payload).unwrap();

    header[0] & 0x0f
}

fn spawn_split_frame_server() -> (String, std_mpsc::Receiver<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (header_tx, header_rx) = std_mpsc::sync_channel(1);

    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        let mut socket = tungstenite::accept(stream).unwrap();
        let payload = EXPECTED.as_bytes();
        assert!(payload.len() < 126);

        // Let tungstenite consume and automatically answer the immediate
        // first Ping before starting the intentionally partial data frame.
        assert!(socket.read().unwrap().is_ping());
        socket.flush().unwrap();

        // Make only the frame header readable. The client parser consumes it
        // and then blocks waiting for the payload.
        socket
            .get_mut()
            .write_all(&[0x81, payload.len() as u8])
            .unwrap();
        socket.get_mut().flush().unwrap();
        header_tx.send(()).unwrap();

        // Do not release the payload until the client's command branch has
        // completed its write. The old implementation necessarily dropped
        // the in-progress read_frame future at this point.
        let command = socket.read().unwrap();
        assert!(command.is_text());
        assert_eq!(command.into_text().unwrap(), INTERRUPT);

        socket.get_mut().write_all(payload).unwrap();
        socket.get_mut().flush().unwrap();
    });

    (format!("ws://{address}"), header_rx, handle)
}

fn spawn_ping_split_frame_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        let mut socket = tungstenite::accept(stream).unwrap();

        // Consume and answer time::interval's immediate first tick before
        // creating the partial inbound frame used by the test.
        assert!(socket.read().unwrap().is_ping());
        socket.flush().unwrap();

        let payload = EXPECTED.as_bytes();
        socket
            .get_mut()
            .write_all(&[0x81, payload.len() as u8])
            .unwrap();
        socket.get_mut().flush().unwrap();

        // The scheduled second Ping proves the timer branch completed before
        // the payload is released. It cancelled the old implementation's
        // pending read but must not disturb the dedicated reader task.
        // Read the trigger Ping directly from TCP so tungstenite does not
        // automatically interleave a Pong between our header and payload.
        assert_eq!(read_raw_client_opcode(socket.get_mut()), 0x9);
        socket.get_mut().write_all(payload).unwrap();
        socket.get_mut().flush().unwrap();
    });

    (format!("ws://{address}"), handle)
}

fn spawn_control_frame_server() -> (String, std_mpsc::Receiver<()>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (done_tx, done_rx) = std_mpsc::sync_channel(1);

    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        let mut socket = tungstenite::accept(stream).unwrap();

        // Consume and answer the callback client's immediate first Ping.
        assert!(socket.read().unwrap().is_ping());
        socket.flush().unwrap();

        let probe = b"exchange-ping".to_vec();
        socket
            .send(tungstenite::Message::Ping(probe.clone()))
            .unwrap();
        let pong = socket.read().unwrap();
        assert!(pong.is_pong());
        assert_eq!(pong.into_data(), probe);

        socket.close(None).unwrap();
        assert!(socket.read().unwrap().is_close());
        done_tx.send(()).unwrap();
    });

    (format!("ws://{address}"), done_rx, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_during_partial_frame_does_not_lose_frame_boundary() {
    let (url, header_rx, server) = spawn_split_frame_server();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Result<String, String>>();

    let message_tx = event_tx.clone();
    let error_tx = event_tx;
    let socket = WebSocketBuilder::new()
        .on_message(move |message| {
            let _ = message_tx.send(Ok(message));
        })
        .on_error(move |error| {
            let _ = error_tx.send(Err(error));
        })
        .connect_with_config(
            &url,
            ClientConfig::new()
                .with_ping_interval(Duration::from_secs(60))
                .with_connect_timeout(Duration::from_secs(1)),
        )
        .await
        .unwrap();

    tokio::task::spawn_blocking(move || header_rx.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap();

    // Let receive_frame consume the two header bytes and park on the payload.
    tokio::time::sleep(Duration::from_millis(50)).await;
    socket.send(INTERRUPT).unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out waiting for message/error callback")
        .expect("callback channel closed");

    assert_eq!(event.expect("partial-frame read was corrupted"), EXPECTED);
    socket.close();
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_during_partial_frame_does_not_lose_frame_boundary() {
    let (url, server) = spawn_ping_split_frame_server();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Result<String, String>>();

    let message_tx = event_tx.clone();
    let error_tx = event_tx;
    let socket = WebSocketBuilder::new()
        .on_message(move |message| {
            let _ = message_tx.send(Ok(message));
        })
        .on_error(move |error| {
            let _ = error_tx.send(Err(error));
        })
        .connect_with_config(
            &url,
            ClientConfig::new()
                .with_ping_interval(Duration::from_millis(100))
                .with_connect_timeout(Duration::from_secs(1)),
        )
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out waiting for message/error callback")
        .expect("callback channel closed");

    assert_eq!(event.expect("partial-frame read was corrupted"), EXPECTED);
    socket.close();
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_reader_preserves_automatic_pong_and_close_reply() {
    let (url, done_rx, server) = spawn_control_frame_server();
    let (close_tx, mut close_rx) = mpsc::unbounded_channel();

    let socket = WebSocketBuilder::new()
        .on_close(move || {
            let _ = close_tx.send(());
        })
        .connect_with_config(
            &url,
            ClientConfig::new()
                .with_ping_interval(Duration::from_secs(60))
                .with_connect_timeout(Duration::from_secs(1)),
        )
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), close_rx.recv())
        .await
        .expect("timed out waiting for close callback")
        .expect("close callback channel closed");
    tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(3)).unwrap())
        .await
        .unwrap();

    socket.close();
    server.join().unwrap();
}
