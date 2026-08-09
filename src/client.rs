use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time;

use crate::HeaderMap;
use crate::OpCode;
use crate::base_client;

type Callback<T> = Box<dyn FnMut(T) + Send>;
type VoidCallback = Box<dyn FnMut() + Send>;

/// Represents various error types that can occur in the WebSocket client.
#[derive(Error, Debug)]
pub enum WebSocketClientError {
    /// Raised when the WebSocket connection fails.
    #[error("WebSocket connection failed: {0}")]
    ConnectionError(String),

    /// Raised when the connection attempt times out.
    #[error("Connection timeout after {0:?}")]
    ConnectionTimeout(Duration),

    /// Raised when sending a message fails.
    #[error("Send failed: {0}")]
    SendError(String),

    /// Raised when receiving a message fails.
    #[error("Receive failed: {0}")]
    ReceiveError(String),

    /// Raised when no connection is established.
    #[error("Not connected")]
    NotConnected,
}

/// Holds callback functions that are invoked on specific WebSocket events.
#[derive(Default)]
struct CallbackSet {
    /// Called when the connection is successfully established.
    on_open: Option<VoidCallback>,
    /// Called when the connection is closed.
    on_close: Option<VoidCallback>,
    /// Called when a interval is reached.
    on_interval: Option<VoidCallback>,
    /// Called when an error occurs.
    on_error: Option<Callback<String>>,
    /// Called when a text message is received.
    on_message: Option<Callback<String>>,
}

impl CallbackSet {
    pub fn call_on_open(&mut self) {
        if let Some(cb) = &mut self.on_open {
            cb();
        }
    }
    pub fn call_on_message(&mut self, message: String) {
        if let Some(cb) = &mut self.on_message {
            cb(message);
        }
    }
    pub fn call_on_error(&mut self, message: String) {
        if let Some(cb) = &mut self.on_error {
            cb(message);
        }
    }
    pub fn call_on_close(&mut self) {
        if let Some(cb) = &mut self.on_close {
            cb();
        }
    }
    pub fn call_on_interval(&mut self) {
        if let Some(cb) = &mut self.on_interval {
            cb();
        }
    }
}

/// Configuration options for the WebSocket client runtime behavior.
pub struct ClientConfig {
    /// Interval at which ping frames are sent.
    ///
    /// **Default**: 30 seconds
    ping_interval: Duration,
    /// Delay before attempting to reconnect after a disconnection.
    ///
    /// Acts as the **initial** backoff when `reconnect_backoff` is enabled,
    /// and as the fixed delay otherwise.
    ///
    /// **Default**: 10 seconds
    reconnect_delay: Duration,
    /// Upper bound for the exponential backoff. Only meaningful when
    /// `reconnect_backoff` is enabled.
    ///
    /// **Default**: 30 seconds
    max_reconnect_delay: Duration,
    /// When `true`, reconnect delay grows exponentially (with jitter) up to
    /// `max_reconnect_delay`. When `false`, every reconnect waits exactly
    /// `reconnect_delay`.
    ///
    /// **Default**: `true`
    reconnect_backoff: bool,
    /// Timeout for connection establishment.
    ///
    /// **Default**: 30 seconds
    connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(30),
            reconnect_backoff: true,
            connect_timeout: Duration::from_secs(30),
        }
    }
}

impl ClientConfig {
    /// Creates a new default configuration.
    ///
    /// - `ping_interval`: 30 seconds  
    /// - `reconnect_delay`: 1 second (initial backoff)
    /// - `max_reconnect_delay`: 30 seconds
    /// - `reconnect_backoff`: enabled
    /// - `connect_timeout`: 30 seconds
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the ping interval duration.
    ///
    /// **Default**: 30 seconds
    pub fn with_ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// Sets the reconnect delay duration.
    ///
    /// When backoff is enabled (default), this is the **initial** backoff
    /// for the first reconnect; subsequent attempts multiply by 2 (capped at
    /// `max_reconnect_delay`) and add a random jitter. When backoff is
    /// disabled, this is the fixed delay between reconnects.
    ///
    /// **Default**: 1 second
    pub fn with_reconnect_delay(mut self, delay: Duration) -> Self {
        self.reconnect_delay = delay;
        self
    }

    /// Sets the upper bound for the exponential backoff.
    ///
    /// Only takes effect when `reconnect_backoff` is enabled.
    ///
    /// **Default**: 30 seconds
    pub fn with_max_reconnect_delay(mut self, delay: Duration) -> Self {
        self.max_reconnect_delay = delay;
        self
    }

    /// Enables or disables exponential backoff with jitter on reconnect.
    ///
    /// When disabled, every reconnect waits exactly `reconnect_delay`.
    /// Disabling backoff is generally a bad idea against a rate-limiting /
    /// WAF-protected endpoint: multiple parallel clients will synchronize
    /// their retries and amplify each other's failures.
    ///
    /// **Default**: `true` (backoff enabled)
    pub fn with_reconnect_backoff(mut self, enabled: bool) -> Self {
        self.reconnect_backoff = enabled;
        self
    }

    /// Sets the connection timeout duration.
    ///
    /// **Default**: 30 seconds
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }
}

/// Options used to initialize a WebSocket connection.
///
/// # Defaults
/// - `vectored`: `true`  
/// - `max_message_size`: `64 MiB`  
/// - `auto_close`: `true`  
/// - `writev_threshold`: `1024`  
/// - `auto_apply_mask`: `true`
/// - `custom_headers`: empty  
#[derive(Clone)]
pub struct ConnectionInitOptions {
    vectored: bool,
    max_message_size: usize,
    auto_close: bool,
    writev_threshold: usize,
    auto_apply_mask: bool,
    custom_headers: HeaderMap,
}

impl Default for ConnectionInitOptions {
    fn default() -> Self {
        Self {
            vectored: true,
            max_message_size: 64 << 20, // 64 MiB
            auto_close: true,
            writev_threshold: 1024,
            auto_apply_mask: true,
            custom_headers: HeaderMap::new(),
        }
    }
}

impl ConnectionInitOptions {
    /// Creates a new options instance with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables vectored writes.
    ///
    /// **Default**: `true`
    pub fn vectored(mut self, value: bool) -> Self {
        self.vectored = value;
        self
    }

    /// Sets the maximum allowed message size in bytes.
    ///
    /// **Default**: `64 MiB`
    pub fn max_message_size(mut self, value: usize) -> Self {
        self.max_message_size = value;
        self
    }

    /// Enables or disables automatic connection closing on close frames.
    ///
    /// **Default**: `true`
    pub fn auto_close(mut self, value: bool) -> Self {
        self.auto_close = value;
        self
    }

    /// Sets the threshold for using vectored I/O writes.
    ///
    /// **Default**: `1024`
    pub fn writev_threshold(mut self, value: usize) -> Self {
        self.writev_threshold = value;
        self
    }

    /// Enables or disables automatic masking of frames.
    ///
    /// **Default**: `true`
    pub fn auto_apply_mask(mut self, value: bool) -> Self {
        self.auto_apply_mask = value;
        self
    }

    /// Sets custom HTTP headers to be included in the handshake.
    ///
    /// **Default**: empty
    pub fn custom_headers(mut self, headers: HeaderMap) -> Self {
        self.custom_headers = headers;
        self
    }
}

// Represents commands sent to the WebSocket client runtime.
pub enum ClientCommand {
    /// Reconnect the connection.
    Reconnect,
    /// Close the connection.
    Close,
    /// Update client configuration.
    UpdateConfig(ClientConfig),
    /// Update connection initialization options.
    UpdateOptions(ConnectionInitOptions),
    /// Send a text message.
    SendMessage(String),
    /// Send multiple text messages contiguously before processing another command.
    SendMessages(Vec<String>),
}

/// Represents a WebSocket client instance.
pub struct WebSocket {
    task_handle: JoinHandle<()>,
    command_tx: mpsc::UnboundedSender<ClientCommand>,
    shutdown_notifier: oneshot::Receiver<()>,
}

impl WebSocket {
    /// Connects to the given URL using the default configuration and returns a new [`WebSocket`] instance.
    ///
    /// This is a convenience method that delegates to [`WebSocketBuilder`].
    ///
    /// # Arguments
    ///
    /// * `url` - A string slice representing the WebSocket server URL.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketClientError`] if the connection attempt fails.
    ///
    /// # Example
    ///
    /// ```
    /// let socket = WebSocket::connect("wss://example.com/socket").await?;
    /// ```
    pub async fn connect(url: &str) -> Result<Self, WebSocketClientError> {
        WebSocketBuilder::new().connect(url).await
    }

    /// Alias for [`WebSocket::connect`]. Initializes a new [`WebSocket`] connection.
    ///
    /// # Arguments
    ///
    /// * `url` - A string slice representing the WebSocket server URL.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketClientError`] if the connection attempt fails.
    ///
    /// # Example
    ///
    /// ```
    /// let socket = WebSocket::new("wss://example.com/socket").await?;
    /// ```
    pub async fn new(url: &str) -> Result<Self, WebSocketClientError> {
        Self::connect(url).await
    }

    /// Updates the client configuration at runtime.
    pub fn update_config(&self, config: ClientConfig) -> &Self {
        let _ = self.command_tx.send(ClientCommand::UpdateConfig(config));
        self
    }

    /// Updates the connection initialization options.
    pub fn update_options(&self, opts: ConnectionInitOptions) -> &Self {
        let _ = self.command_tx.send(ClientCommand::UpdateOptions(opts));
        self
    }

    /// Sends a command to close the connection.
    pub fn close(&self) -> &Self {
        let _ = self.command_tx.send(ClientCommand::Close);
        self
    }

    /// Awaits shutdown of the WebSocket task.
    pub async fn await_shutdown(self) {
        let _ = self.shutdown_notifier.await;
        let _ = self.task_handle.await;
    }

    /// Joins the WebSocket task and returns its result.
    /// This is an alias for the task_handle.await operation.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.task_handle.await
    }

    /// Sends a text message over the connection.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketClientError::SendError`] if the message could not be sent.
    pub fn send(&self, message: &str) -> Result<(), WebSocketClientError> {
        self.command_tx
            .send(ClientCommand::SendMessage(message.to_string()))
            .map_err(|e| WebSocketClientError::SendError(e.to_string()))
    }

    /// Enqueues multiple text messages as one writer command.
    ///
    /// The runtime holds the writer lock until every message in this burst has
    /// been written, so frames from another task cannot be interleaved.
    pub fn send_many<I, S>(&self, messages: I) -> Result<(), WebSocketClientError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let messages = messages.into_iter().map(Into::into).collect::<Vec<_>>();
        if messages.is_empty() {
            return Ok(());
        }
        self.command_tx
            .send(ClientCommand::SendMessages(messages))
            .map_err(|e| WebSocketClientError::SendError(e.to_string()))
    }
    pub fn send_command(&self, command: ClientCommand) -> Result<(), WebSocketClientError> {
        self.command_tx
            .send(command)
            .map_err(|e| WebSocketClientError::SendError(e.to_string()))
    }
}

/// Builder for creating and connecting a WebSocket client with callbacks and options.
pub struct WebSocketBuilder {
    callbacks: CallbackSet,
    options: ConnectionInitOptions,
    command_rx: Option<mpsc::UnboundedReceiver<ClientCommand>>,
    command_tx: Option<mpsc::UnboundedSender<ClientCommand>>,
}

impl Default for WebSocketBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSocketBuilder {
    /// Creates a new WebSocketBuilder with default options.
    pub fn new() -> Self {
        Self {
            callbacks: CallbackSet::default(),
            options: ConnectionInitOptions::default(),
            command_rx: None,
            command_tx: None,
        }
    }
    pub fn with_command_channel(
        mut self,
        command_rx: mpsc::UnboundedReceiver<ClientCommand>,
        command_tx: mpsc::UnboundedSender<ClientCommand>,
    ) -> Self {
        self.command_rx = Some(command_rx);
        self.command_tx = Some(command_tx);
        self
    }
    pub fn on_interval<F>(mut self, f: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        self.callbacks.on_interval = Some(Box::new(f));
        self
    }

    /// Registers a callback to be invoked when the connection opens.
    pub fn on_open<F>(mut self, f: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        self.callbacks.on_open = Some(Box::new(f));
        self
    }

    /// Registers a callback to be invoked when the connection closes.
    pub fn on_close<F>(mut self, f: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        self.callbacks.on_close = Some(Box::new(f));
        self
    }

    /// Registers a callback to be invoked when an error occurs.
    pub fn on_error<F>(mut self, f: F) -> Self
    where
        F: FnMut(String) + Send + 'static,
    {
        self.callbacks.on_error = Some(Box::new(f));
        self
    }

    /// Registers a callback to be invoked when a message is received.
    pub fn on_message<F>(mut self, f: F) -> Self
    where
        F: FnMut(String) + Send + 'static,
    {
        self.callbacks.on_message = Some(Box::new(f));
        self
    }

    /// Sets the connection initialization options.
    pub fn with_options(mut self, options: ConnectionInitOptions) -> Self {
        self.options = options;
        self
    }

    /// Connects to the given URL using default client configuration.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketClientError`] if the connection could not be established.
    pub async fn connect(self, url: &str) -> Result<WebSocket, WebSocketClientError> {
        self.connect_with_config(url, ClientConfig::new()).await
    }

    /// Connects to the given URL using a specific client configuration.
    ///
    /// # Errors
    ///
    /// Returns [`WebSocketClientError`] if the connection could not be established.
    pub async fn connect_with_config(
        mut self,
        url: &str,
        config: ClientConfig,
    ) -> Result<WebSocket, WebSocketClientError> {
        if self.command_rx.is_none() {
            let (command_tx, command_rx) = mpsc::unbounded_channel();
            self.command_rx = Some(command_rx);
            self.command_tx = Some(command_tx);
        }
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let url = url.to_owned();
        let task_handle = tokio::spawn(async move {
            run(
                &url,
                config,
                self.options,
                self.callbacks,
                self.command_rx.unwrap(),
            )
            .await;
            let _ = shutdown_tx.send(());
        });

        Ok(WebSocket {
            command_tx: self.command_tx.take().unwrap(),
            task_handle,
            shutdown_notifier: shutdown_rx,
        })
    }
}

const READER_EVENT_BUFFER: usize = 1024;

enum ReaderEvent {
    Text(String),
    Close,
    Error(String),
}

async fn read_frames(
    mut reader: base_client::OnlineReader,
    writer: base_client::SharedOnlineWriter,
    event_tx: mpsc::Sender<ReaderEvent>,
) {
    loop {
        match reader.receive_frame(&writer).await {
            Ok(frame) => {
                let event = match frame.opcode {
                    OpCode::Close => ReaderEvent::Close,
                    OpCode::Text => match String::from_utf8(frame.payload.into()) {
                        Ok(text) => ReaderEvent::Text(text),
                        Err(error) => ReaderEvent::Error(error.to_string()),
                    },
                    _ => continue,
                };

                let should_stop = matches!(event, ReaderEvent::Close | ReaderEvent::Error(_));
                if event_tx.send(event).await.is_err() || should_stop {
                    break;
                }
            }
            Err(error) => {
                let _ = event_tx.send(ReaderEvent::Error(error.to_string())).await;
                break;
            }
        }
    }
}

async fn run(
    url: &str,
    mut config: ClientConfig,
    mut options: ConnectionInitOptions,
    mut callbacks: CallbackSet,
    mut command_rx: mpsc::UnboundedReceiver<ClientCommand>,
) {
    let mut shutdown = false;
    // Number of consecutive failed connect attempts. Reset to 0 as soon as the
    // connection is successfully established. Used to compute exponential
    // backoff on the next reconnect.
    let mut consecutive_failures: u32 = 0;

    while !shutdown {
        // Why we exited the previous iteration. Used to decide whether the
        // trailing sleep should be a backoff (real failure / unexpected drop)
        // or a short, fixed delay (user-requested Reconnect command).
        let mut was_failure = true;

        match try_connect(url, &options, config.connect_timeout).await {
            Ok(client) => {
                // Connection succeeded — reset the failure counter so that a
                // future transient drop doesn't immediately jump to the cap.
                consecutive_failures = 0;
                let (reader, writer) = client.into_split();

                // Consume and apply all pending `update` commands, including immediately following callback registrations
                let message = loop {
                    tokio::task::yield_now().await;
                    match command_rx.try_recv() {
                        Ok(ClientCommand::Reconnect) => {
                            let result = writer.lock().await.send_close("").await;
                            if let Err(error) = result {
                                callbacks.call_on_error(error.to_string());
                            }
                            was_failure = false;
                            break None;
                        }
                        Ok(ClientCommand::Close) => {
                            let result = writer.lock().await.send_close("").await;
                            if let Err(error) = result {
                                callbacks.call_on_error(error.to_string());
                            }
                            shutdown = true;
                            break None;
                        }
                        Ok(ClientCommand::UpdateConfig(cfg)) => {
                            config = cfg;
                        }
                        Ok(ClientCommand::UpdateOptions(opts)) => {
                            options = opts;
                        }
                        Ok(ClientCommand::SendMessage(message)) => break Some(vec![message]),
                        Ok(ClientCommand::SendMessages(messages)) => break Some(messages),
                        Err(mpsc::error::TryRecvError::Empty) => break None,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            shutdown = true;
                            log::error!("command_rx disconnected, shutting down");
                            break None;
                        }
                    }
                };
                callbacks.call_on_open();
                let mut initial_send_failed = false;
                if let Some(messages) = message {
                    let mut writer_guard = writer.lock().await;
                    for message in messages {
                        if let Err(error) = writer_guard.send_string(&message).await {
                            callbacks.call_on_error(error.to_string());
                            initial_send_failed = true;
                            break;
                        }
                    }
                }
                if shutdown {
                    callbacks.call_on_close();
                    break;
                }

                if !initial_send_failed {
                    let (reader_event_tx, mut reader_event_rx) = mpsc::channel(READER_EVENT_BUFFER);
                    let reader_task =
                        tokio::spawn(read_frames(reader, Arc::clone(&writer), reader_event_tx));
                    let mut ping_timer = time::interval(config.ping_interval);

                    loop {
                        tokio::select! {
                            _ = ping_timer.tick() => {
                                let result = writer.lock().await.send_ping("").await;
                                if let Err(error) = result {
                                    callbacks.call_on_error(error.to_string());
                                    break;
                                }
                                callbacks.call_on_interval();
                            }
                            command = command_rx.recv() => {
                                let Some(command) = command else {
                                    shutdown = true;
                                    log::error!("command_rx disconnected, shutting down");
                                    break;
                                };

                                match command {
                                    ClientCommand::Reconnect => {
                                        let result = writer.lock().await.send_close("").await;
                                        if let Err(error) = result {
                                            callbacks.call_on_error(error.to_string());
                                        }
                                        // Explicit, user-initiated reconnect — do not back off.
                                        was_failure = false;
                                        break;
                                    },
                                    ClientCommand::Close => {
                                        let result = writer.lock().await.send_close("").await;
                                        if let Err(error) = result {
                                            callbacks.call_on_error(error.to_string());
                                        }
                                        callbacks.call_on_close();
                                        shutdown = true;
                                        break;
                                    },
                                    ClientCommand::UpdateConfig(cfg) => {
                                        config = cfg;
                                        ping_timer = time::interval(config.ping_interval);
                                    },
                                    ClientCommand::UpdateOptions(opts) => {
                                        options = opts;
                                    },
                                    ClientCommand::SendMessage(message) => {
                                        let result = writer.lock().await.send_string(&message).await;
                                        if let Err(error) = result {
                                            callbacks.call_on_error(error.to_string());
                                            break;
                                        }
                                    },
                                    ClientCommand::SendMessages(messages) => {
                                        let mut writer_guard = writer.lock().await;
                                        let mut failed = false;
                                        for message in messages {
                                            if let Err(error) = writer_guard.send_string(&message).await {
                                                callbacks.call_on_error(error.to_string());
                                                failed = true;
                                                break;
                                            }
                                        }
                                        if failed {
                                            break;
                                        }
                                    },
                                }
                            }
                            event = reader_event_rx.recv() => {
                                match event {
                                    Some(ReaderEvent::Text(text)) => {
                                        callbacks.call_on_message(text);
                                    }
                                    Some(ReaderEvent::Close) => {
                                        callbacks.call_on_close();
                                        break;
                                    }
                                    Some(ReaderEvent::Error(error)) => {
                                        callbacks.call_on_error(error);
                                        break;
                                    }
                                    None => {
                                        callbacks.call_on_error(
                                            "WebSocket reader task stopped unexpectedly".to_string(),
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // Cancelling a read is safe only because the entire
                    // connection and its parser state are discarded below.
                    reader_task.abort();
                    let _ = reader_task.await;
                }
            }
            Err(e) => {
                callbacks.call_on_error(e.to_string());
            }
        }

        // Only sleep for reconnect delay if we're not shutting down
        if !shutdown {
            let delay = if was_failure {
                consecutive_failures = consecutive_failures.saturating_add(1);
                compute_backoff(&config, consecutive_failures)
            } else {
                // Explicit Reconnect command — reconnect ASAP.
                config.reconnect_delay.min(Duration::from_millis(500))
            };
            log::info!(
                "reconnect backoff: sleeping {:?} (failure #{})",
                delay,
                if was_failure { consecutive_failures } else { 0 }
            );
            time::sleep(delay).await;
        }
    }
}

/// Compute the next reconnect delay, applying exponential backoff with jitter
/// when `reconnect_backoff` is enabled, or the fixed `reconnect_delay`
/// otherwise.
///
/// Backoff sequence (without jitter) for the default config
/// (`reconnect_delay=1s`, `max_reconnect_delay=30s`):
///
/// ```text
/// failure #1 → 1s
/// failure #2 → 2s
/// failure #3 → 4s
/// failure #4 → 8s
/// failure #5 → 16s
/// failure #6 → 30s (capped)
/// ```
///
/// On top of the base delay we add a uniformly random jitter in `[0, base/2]`
/// so that multiple clients (or multiple sockets in the same process) that
/// happen to fail at the same time don't synchronise their retries and
/// amplify a rate-limit / WAF condition. The jitter is seeded from
/// `SystemTime` plus the failure counter and the address of `config`, which
/// avoids pulling in a `rand` dependency while still giving good
/// decorrelation between sockets.
fn compute_backoff(config: &ClientConfig, failure_count: u32) -> Duration {
    let base = config.reconnect_delay;
    let cap = config.max_reconnect_delay.max(base);

    if !config.reconnect_backoff || failure_count == 0 {
        return base;
    }

    // Doubling backoff: base * 2^(n-1). `saturating_mul` keeps us safe from
    // overflow; the `min(cap)` afterwards makes the cap the effective limit.
    let mut exp = base;
    for _ in 1..failure_count {
        exp = match exp.checked_mul(2) {
            Some(v) => v,
            None => break,
        };
    }
    let base_delay = exp.min(cap);

    // Jitter: uniformly distributed in [0, base_delay / 2].
    //
    // Why jitter? Without it, multiple sockets (or multiple sockets in the
    // same process) that fail at the same instant will retry at the same
    // instant too — and against a rate-limit / WAF-protected endpoint that
    // synchronised retry *amplifies* the original failure (see e.g. the
    // CloudFront 403 incident on bitget when several private sockets all
    // reconnect every 10s in lockstep). A small amount of jitter decorrelates
    // them.
    //
    // We use `SystemTime` nanos XOR'd with the address of `config` and the
    // failure count as the entropy source. That's enough for decorrelation
    // without pulling in a `rand` dependency.
    let seed_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cfg_addr = std::ptr::addr_of!(config) as usize as u128;
    let mix = (seed_ns ^ cfg_addr ^ failure_count as u128).wrapping_mul(0x9E3779B97F4A7C15);
    let jitter_max = base_delay.as_nanos() / 2;
    let jitter_ns = if jitter_max == 0 {
        0
    } else {
        (mix % jitter_max) as u64
    };

    base_delay + Duration::from_nanos(jitter_ns)
}

async fn try_connect(
    url: &str,
    options: &ConnectionInitOptions,
    timeout: Duration,
) -> Result<base_client::Online, WebSocketClientError> {
    let mut offline = base_client::Offline::new();
    offline
        .set_writev(options.vectored)
        .set_writev_threshold(options.writev_threshold)
        .set_auto_close(options.auto_close)
        .set_max_message_size(options.max_message_size)
        .set_auto_apply_mask(options.auto_apply_mask);

    for (k, v) in options.custom_headers.iter() {
        offline.add_header(k.clone(), v.clone());
    }

    tokio::time::timeout(timeout, offline.connect(url))
        .await
        .map_err(|_| WebSocketClientError::ConnectionTimeout(timeout))?
        .map_err(|e| WebSocketClientError::ConnectionError(e.to_string()))
}
