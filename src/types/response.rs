use crate::types::HassEvent;
use crate::HassResult;

use serde::Deserialize;
use serde_json::Value;

///The tag identifying which variant we are dealing with is inside of the content,
/// next to any other fields of the variant.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Response {
    /// Authentication messages have no id field
    Auth(AuthResponse),
    /// A response message
    OneShot(OneShotResponse),
    /// An event on a subscription channel
    Event(WSEvent),
}

impl Response {
    pub fn id(&self) -> Option<u64> {
        match self {
            Response::Auth(_) => None,
            Response::OneShot(msg) => Some(msg.id()),
            Response::Event(event) => Some(event.id),
        }
    }
    pub fn into_auth(self) -> Result<AuthResponse, Self> {
        match self {
            Response::Auth(auth) => Ok(auth),
            other => Err(other),
        }
    }
    pub fn into_oneshot(self) -> Result<OneShotResponse, Self> {
        match self {
            Response::OneShot(oneshot) => Ok(oneshot),
            other => Err(other),
        }
    }
    pub fn into_event(self) -> Result<WSEvent, Self> {
        match self {
            Response::Event(event) => Ok(event),
            other => Err(other),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthResponse {
    /// Request to authenticate
    AuthRequired(AuthRequired),
    /// Authentication succeeded
    #[allow(unused)]
    AuthOk(AuthOk),
    /// Authentication failed
    AuthInvalid(AuthInvalid),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OneShotResponse {
    /// General response from server
    Result(WSResult),
    /// Response to ping request
    Pong(WSPong),
}

impl OneShotResponse {
    pub fn id(&self) -> u64 {
        match self {
            OneShotResponse::Pong(pong) => pong.id,
            OneShotResponse::Result(result) => result.id,
        }
    }
}

// this is the first message received from websocket,
// that ask to provide a authetication method
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AuthRequired {
    pub ha_version: String,
}

// this is received when the service successfully autheticate
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AuthOk {
    pub ha_version: String,
}

// this is received if the authetication failed
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AuthInvalid {
    pub message: String,
}

// this is received as a response to a ping request
#[derive(Debug, Deserialize, PartialEq)]
pub struct WSPong {
    pub id: u64,
}

///	This object represents the Home Assistant Event
///
/// received when the client is subscribed to
/// [Subscribe to events](https://developers.home-assistant.io/docs/api/websocket/#subscribe-to-events)
#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct WSEvent {
    pub id: u64,
    pub event: HassEvent,
}

///this is the general response from the Websocket server when a requesthas been sent
///
/// if "success" is true, then the "result" can be checked
/// if "suceess" is false, then the "error" should be further explored
#[derive(Debug, Deserialize, PartialEq)]
pub struct WSResult {
    pub id: u64,
    success: bool,
    result: Option<Value>,
    error: Option<ErrorCode>,
}

impl WSResult {
    pub fn is_ok(&self) -> bool {
        self.success
    }

    pub fn is_err(&self) -> bool {
        !self.success
    }

    pub fn result(self) -> HassResult<Value> {
        if self.success {
            if let Some(result) = self.result {
                return Ok(result);
            }
        }
        Err(crate::HassError::ResponseError(self))
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct ErrorCode {
    pub code: String,
    pub message: String,
}
