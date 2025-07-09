use anyhow::{anyhow, Result};
use hass_rs::client::HassClient;
use tokio::sync::Mutex;
use std::env::var;
use std::sync::Arc;
use std::{thread, time};

// This is just a mock token manager. In a real app, you would want something that
// actually performs OAuth so you can ultimately return an `Option<String>`.
#[derive(Clone, Default)]
struct TokenManager {
    /// This counter is here to demonstrate how this token provider can have state
    /// that is modified from more than one thread, such as when it is used to
    /// manage tokens for both the websocket interface and another component
    /// that needs to authenticate with Home Assistant.
    counter: usize
}
impl TokenManager {
    fn get_token(&mut self) -> Option<String> {
        self.counter += 1;
        println!("A token has been requested {} time so far", self.counter);

        var("HASS_TOKEN").ok()
            .or_else(|| {
                println!("Please set the HASS_TOKEN env variable before running this");
                None
            })
    }
}

struct State {
    client: HassClient,
    tokens: Arc<Mutex<TokenManager>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let url = "ws://localhost:8123/api/websocket";

    // The token manager is being created as an Arc<Mutex> so that it can be shared
    // between threads, even though it isn't actually used in another thread.
    let tokens = Arc::new(Mutex::new(TokenManager::default()));
    println!("Connecting to - {}", url);

    // The closure that provides the authentication token is a bit more complex
    // than most closures because it is not allowed to capture any references
    // from its environment. It must take ownership of everything it captures.
    // This is a necessary restriction because the closure is stored inside the
    // `HassClient` so it can run the closure every time it needs a new token.
    // To make this possible, we create a closure that takes ownership of whatever
    // it needs before opening an async block so the closure returns a `Future`.
    let hass_tokens = Arc::clone(&tokens);
    let client = HassClient::new_with_auth(url,
        move || {
            let tokens = hass_tokens.clone();
            async move {
                println!("HassClient requested a token.");
                tokens.lock().await.get_token()
            }
        }).await?;

    let mut state = State {
        client,
        tokens
    };

    println!("WebSocket connection and authentication works");

    println!("Get another token from the same token provider");
    // Ignore the actual token value because we don't want to print it to the console.
    let _ = state.tokens.lock().await.get_token().ok_or_else(|| anyhow!("Token provider has no token for us."))?;

    println!("Now, trigger a reconnect just to prove tokens come from the same provider");
    state.client.reconnect().await?;

    println!("Subscribe to an Event");

    let mut event_receiver = state.client
        .subscribe_event("state_changed")
        .await?;

    // Spawn a Tokio task to do whatever we want with the received events
    tokio::spawn(async move {
        while let Some(message) = event_receiver.recv().await {
            println!("Event Received: {:?}", message);
        }
    });

    thread::sleep(time::Duration::from_secs(20));

    Ok(())
}


// In order to Test go to Home Assistant --> Developer Tools --> Events , and fire the selected test Event
//
// Connecting to - ws://localhost:8123/api/websocket
// HassClient requested a token.
// A token has been requested 1 time so far
// WebSocket connection and authentication works
// Get another token from the same token provider
// A token has been requested 2 time so far
// Now, trigger a reconnect just to prove tokens come from the same provider
// HassClient requested a token.
// A token has been requested 3 time so far
// Subscribe to an Event
// Event subscribed: WSResult { id: 1, success: true, result: None, error: None }
//
// Event Received: WSEvent { id: 1, event: HassEvent { data: EventData { entity_id: None, new_state: None, old_state: None }, event_type: "state_changed", time_fired: "2024-02-16T09:46:45.013050+00:00", origin: "REMOTE", context: Context { id: "01HPRMZAWNXKVVPSP11QFJ53HB", parent_id: None, user_id: Some("f069978dd7964042824cb09287fe7c73") } } }
// Event Received: WSEvent { id: 1, event: HassEvent { data: EventData { entity_id: None, new_state: None, old_state: None }, event_type: "state_changed", time_fired: "2024-02-16T09:46:46.038355+00:00", origin: "REMOTE", context: Context { id: "01HPRMZBWP8E5HQFNV60CJ9HB1", parent_id: None, user_id: Some("f069978dd7964042824cb09287fe7c73") } } }
// Event Received: WSEvent { id: 1, event: HassEvent { data: EventData { entity_id: None, new_state: None, old_state: None }, event_type: "state_changed", time_fired: "2024-02-16T09:46:57.997747+00:00", origin: "REMOTE", context: Context { id: "01HPRMZQJDCEHT1PRQKK6H96AH", parent_id: None, user_id: Some("f069978dd7964042824cb09287fe7c73") } } }
//
// Unsubscribe the Event
// Succefully unsubscribed: Ok
