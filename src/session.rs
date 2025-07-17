use crate::types::{
    Command, Response,
    WSEvent,
};
use crate::{Auth, AuthResponse, HassError, HassResult, OneShotResponse};

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Payload;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::oneshot::{channel as oneshot, Sender as OneShotSender};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

pub(crate) struct Session {
    // holds the id of the WS message
    last_sequence: AtomicU64,

    // rx_state: Arc<ReceiverState>,
    subscriptions: Mutex<HashMap<u64, Sender<WSEvent>>>,
    pending_requests: Mutex<HashMap<u64, OneShotSender<OneShotResponse>>>,

    /// Client --> Gateway (send "Commands" msg to the Gateway)
    // message_tx: Mutex<Sender<Message>>,
    sink: AsyncMutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
}

impl Session {
    pub(crate) async fn connect(url: &str, token: &str) -> HassResult<Arc<Self>> {
        let (wsclient, _) = connect_async(url).await?;
        let (mut sink, mut stream) = wsclient.split();

        let message = stream.next().await.ok_or_else(|| HassError::ConnectionClosed)??;
        let text = message.into_text().map_err(|_| HassError::Generic("Initial message from server was not text".to_string()))?;
        let response: AuthResponse = serde_json::from_str(text.as_str())?;
        if !matches!(response, AuthResponse::AuthRequired(_)) {
            return Err(HassError::UnknownPayloadReceived(Response::Auth(response)));
        }

        // let (message_tx, mut message_rx) = channel(20);

        let auth_message = Command::AuthInit(Auth {
            msg_type: "auth".to_owned(),
            access_token: token.to_owned(),
        });
        let cmd_tungstenite = auth_message.to_tungstenite_message();

        // Send the auth command to gateway
        sink
            .send(cmd_tungstenite)
            .await
            .map_err(|err| HassError::SendError(err.to_string()))?;

        let message = stream.next().await.ok_or_else(|| HassError::ConnectionClosed)??;
        let text = message.into_text().map_err(|_| HassError::Generic("Initial message from server was not text".to_string()))?;
        let response: AuthResponse = serde_json::from_str(text.as_str())?;

        // Check if the authentication was successful, should receive {"type": "auth_ok"}
        match response {
            AuthResponse::AuthOk(_) => {},
            AuthResponse::AuthInvalid(err) => Err(HassError::AuthenticationFailed(err.message))?,
            unknown => Err(HassError::UnknownPayloadReceived(Response::Auth(unknown)))?,
        }

        let last_sequence = AtomicU64::new(1);
        let this = Arc::new(Self {
            last_sequence,
            sink: AsyncMutex::new(sink),
            // message_tx: Mutex::new(message_tx),
            subscriptions: Default::default(),
            pending_requests: Default::default(),
        });

        let background_this = Arc::clone(&this);
        tokio::spawn(async move {
            while let Some(message) = stream.next().await {
                log::trace!("incoming: {message:#?}");

                match message {
                    Ok(Message::Text(data)) => {
                        background_this.handle_incoming_text(data).await;
                    }
                    Ok(Message::Ping(data)) => {
                        background_this.send_message(Message::Pong(data)).await;
                    }
                    unexpected => log::error!("Unexpected message: {unexpected:#?}"),
                }
            }
        });

        Ok(this)
    }
    pub(crate) async fn disconnect(self: &Arc<Self>) -> HassResult<()> {
        let mut sink = self.sink.lock().await;
        Ok(sink.close().await?)
    }
    pub(crate) async fn dispatch_event(self: &Arc<Self>, event: WSEvent) {
        // Dispatch to subscriber
        let id = event.id;
        if let Some(tx) = self.get_tx(id) {
            if tx.send(event).await.is_err() {
                self.rm_subscription(id);
                // TODO: send unsub request here
            }
        }
    }
    pub(crate) fn resolve_oneshot(self: &Arc<Self>, oneshot: OneShotResponse) {
        let id = oneshot.id();
        if let Some(tx) = self.take_responder(id) {
            tx.send(oneshot).ok();
        } else {
            log::error!("no responder for id={id} {oneshot:#?}");
        }
    }
    pub(crate) fn get_tx(self: &Arc<Self>, id: u64) -> Option<Sender<WSEvent>> {
        self.subscriptions.lock().get(&id).cloned()
    }
    /// This method creates a subscription channel and returns the receiver half of it.
    ///
    /// If there is an existing channel with the same id, it is dropped with a warning.
    pub(crate) fn get_rx(self: &Arc<Self>, id: u64) -> Receiver<WSEvent> {
        let (tx, rx) = channel(20);
        self.subscriptions.lock().insert(id, tx);
        rx
    }

    async fn send_message(self: &Arc<Self>, message: Message) {
        if let Err(err) = self.sink.lock().await.send(message).await {
            log::error!("Error sending message: {err:#}");
        }
    }

    pub(crate) fn rm_subscription(self: &Arc<Self>, id: u64) {
        self.subscriptions.lock().remove(&id);
    }

    pub(crate) fn take_responder(self: &Arc<Self>, id: u64) -> Option<OneShotSender<OneShotResponse>> {
        self.pending_requests.lock().remove(&id)
    }

    /// send commands and receive responses from the gateway
    pub(crate) async fn command(self: &Arc<Self>, cmd: &Command, id: u64) -> HassResult<OneShotResponse> {
        let cmd_tungstenite = cmd.to_tungstenite_message();

        let (tx, rx) = oneshot();

        self.pending_requests.lock().insert(id, tx);

        // Send the auth command to gateway
        self.sink
            .lock().await
            .send(cmd_tungstenite)
            .await
            .map_err(|err| HassError::SendError(err.to_string()))?;

        rx.await
            .map_err(|err| HassError::RecvError(err.to_string()))
    }

    async fn handle_incoming_text(self: &Arc<Self>, data: Utf8Payload) {
        match serde_json::from_str(data.as_str()) {
            Ok(Response::Event(event)) => {
                self.dispatch_event(event).await;
            }
            Ok(Response::OneShot(oneshot)) => {
                self.resolve_oneshot(oneshot);
            }
            Ok(Response::Auth(auth)) => {
                log::warn!("Received an auth response after handshake: {auth:?}");
            }
            Err(err) => {
                log::error!("Error deserializing response: {err:#} {data}");
            }
        }
    }

    /// get message sequence required by the Websocket server
    pub(crate) fn next_seq(&self) -> u64 {
        self.last_sequence.fetch_add(1, Ordering::Relaxed)
    }
}
