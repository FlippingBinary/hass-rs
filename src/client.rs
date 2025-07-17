//! Home Assistant client implementation

use crate::types::{
    Ask, CallService, Command, HassConfig, HassEntity, HassPanels, HassServices, Response,
    HassRegistryArea, HassRegistryDevice, HassRegistryEntity,
    Subscribe, WSEvent,
};
use crate::session::Session;
use crate::{HassError, HassResult, OneShotResponse};

use serde_json::Value;
use tokio::sync::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

/// HassClient is a library that is meant to simplify the conversation with HomeAssistant Web Socket Server
/// it provides a number of convenient functions that creates the requests and read the messages from server
pub struct HassClient {
    url: String,
    token_provider: Arc<Mutex<Box<TokenProvider>>>,
    session: Option<Arc<Session>>,
}
type TokenProvider = dyn FnMut() -> Pin<Box<FutureToken>> + Sync + Send;
type FutureToken = dyn Future<Output = Option<String>> + Sync + Send;

impl HassClient {
    pub async fn new(url: &str) -> HassResult<Self> {
        Self::new_with_auth(url, || async {None}).await
    }

    pub async fn new_with_auth<F>(
        url: impl ToString,
        mut token_provider: impl (FnMut() -> F) + Send + Sync + 'static,
    ) -> HassResult<Self>
    where
        F: Future<Output = Option<String>> + Send + Sync + 'static,
    {
        let url = url.to_string();
        let session = if let Some(token) = token_provider().await {
            let session = Session::connect(&url, &token).await?;
            Some(session)
        } else {
            None
        };
        Ok(Self {
            url,
            token_provider: Arc::new(Mutex::new(Box::new(move || {
                Box::pin(token_provider()) as Pin<Box<FutureToken>>
            }))),
            session,
        })
    }

    pub async fn reconnect(&mut self) -> HassResult<()> {
        if let Some(token) = self.get_new_token().await {
            let session = Session::connect(&self.url, &token).await?;
            self.session.replace(session);
        } else {
            self.session.take();
        }
        Ok(())
    }

    pub async fn disconnect(&mut self) -> HassResult<()> {
        let _ = self.session.take();
        if let Some(session) = self.session.take() {
            session.disconnect().await?;
        }
        Ok(())
    }

    /// authenticate the session using a long-lived access token
    ///
    /// When a client connects to the server, the server sends out auth_required.
    /// The first message from the client should be an auth message. You can authorize with an access token.
    /// If the client supplies valid authentication, the authentication phase will complete by the server sending the auth_ok message.
    /// If the data is incorrect, the server will reply with auth_invalid message and disconnect the session.
    pub async fn auth_with_longlivedtoken(&mut self, token: &str) -> HassResult<()> {
        let token = token.to_string();
        (*self.token_provider.lock().await) = Box::new(move || {
            let token = token.to_owned();
            Box::pin(async move {Some(token)}) as Pin<Box<FutureToken>>
        });
        self.reconnect().await
    }

    async fn get_new_token(&self) -> Option<String> {
        ((self.token_provider.lock().await)()).await
    }

    async fn session(&mut self) -> HassResult<&Arc<Session>> {
        if self.session.is_none() {
            self.reconnect().await?;
        }
        self.session.as_ref()
            .ok_or(HassError::SendError("Session does not exist".to_string()))
    }
    pub(crate) async fn command(&mut self, cmd: &Command, id: u64) -> HassResult<OneShotResponse> {
        self.session().await?.command(cmd, id).await
    }

    /// The API supports receiving a ping from the client and returning a pong.
    /// This serves as a heartbeat to ensure the connection is still alive.
    pub async fn ping(&mut self) -> HassResult<()> {
        let id = self.session().await?.next_seq();

        let ping_req = Command::Ping(Ask {
            id,
            msg_type: "ping".to_owned(),
        });

        let response = self.command(&ping_req, id).await?;

        match response {
            OneShotResponse::Pong(_v) => Ok(()),
            OneShotResponse::Result(err) => Err(HassError::ResponseError(err)),
        }
    }

    /// This will get the current config of the Home Assistant.
    ///
    /// The server will respond with a result message containing the config.
    pub async fn get_config(&mut self) -> HassResult<HassConfig> {
        let id = self.session().await?.next_seq();

        let config_req = Command::GetConfig(Ask {
            id,
            msg_type: "get_config".to_owned(),
        });
        let response = self.command(&config_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let config: HassConfig = serde_json::from_value(value)?;
                Ok(config)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// This will get all the current states from Home Assistant.
    ///
    /// The server will respond with a result message containing the states.
    pub async fn get_states(&mut self) -> HassResult<Vec<HassEntity>> {
        let id = self.session().await?.next_seq();

        let states_req = Command::GetStates(Ask {
            id,
            msg_type: "get_states".to_owned(),
        });
        let response = self.command(&states_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let states: Vec<HassEntity> = serde_json::from_value(value)?;
                Ok(states)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// This will get all the services from Home Assistant.
    ///
    /// The server will respond with a result message containing the services.
    pub async fn get_services(&mut self) -> HassResult<HassServices> {
        let id = self.session().await?.next_seq();
        let services_req = Command::GetServices(Ask {
            id,
            msg_type: "get_services".to_owned(),
        });
        let response = self.command(&services_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let services: HassServices = serde_json::from_value(value)?;
                Ok(services)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// This will get all the registered panels from Home Assistant.
    ///
    /// The server will respond with a result message containing the current registered panels.
    pub async fn get_panels(&mut self) -> HassResult<HassPanels> {
        let id = self.session().await?.next_seq();

        let services_req = Command::GetPanels(Ask {
            id,
            msg_type: "get_panels".to_owned(),
        });
        let response = self.command(&services_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let services: HassPanels = serde_json::from_value(value)?;
                Ok(services)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// This will get the current area registry list from Home Assistant.
    ///
    /// The server will respond with a result message containing the area registry list.
    pub async fn get_area_registry_list(&mut self) -> HassResult<Vec<HassRegistryArea>> {
        let id = self.session().await?.next_seq();

        let area_req = Command::GetAreaRegistryList(Ask {
            id,
            msg_type: "config/area_registry/list".to_owned(),
        });
        let response = self.command(&area_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let areas: Vec<HassRegistryArea> = serde_json::from_value(value)?;
                Ok(areas)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// This will get the current device registry list from Home Assistant.
    ///
    /// The server will respond with a result message containing the device registry list.
    pub async fn get_device_registry_list(&mut self) -> HassResult<Vec<HassRegistryDevice>> {
        let id = self.session().await?.next_seq();

        let device_req = Command::GetDeviceRegistryList(Ask {
            id,
            msg_type: "config/device_registry/list".to_owned(),
        });
        let response = self.command(&device_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let devices: Vec<HassRegistryDevice> = serde_json::from_value(value)?;
                Ok(devices)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// This will get the current entity registry list from Home Assistant.
    ///
    /// The server will respond with a result message containing the entity registry list.
    pub async fn get_entity_registry_list(&mut self) -> HassResult<Vec<HassRegistryEntity>> {
        let id = self.session().await?.next_seq();

        let entity_req = Command::GetEntityRegistryList(Ask {
            id,
            msg_type: "config/entity_registry/list".to_owned(),
        });
        let response = self.command(&entity_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                let value = data.result()?;
                let entities: Vec<HassRegistryEntity> = serde_json::from_value(value)?;
                Ok(entities)
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    ///This will call a service in Home Assistant. Right now there is no return value.
    ///The client can listen to state_changed events if it is interested in changed entities as a result of a service call.
    ///
    /// The server will indicate with a message indicating that the service is done executing.
    /// <https://developers.home-assistant.io/docs/api/websocket#calling-a-service>
    /// additional info : <https://developers.home-assistant.io/docs/api/rest> ==> Post `/api/services/<domain>/<service>`
    pub async fn call_service(
        &mut self,
        domain: String,
        service: String,
        service_data: Option<Value>,
    ) -> HassResult<()> {
        let id = self.session().await?.next_seq();

        let services_req = Command::CallService(CallService {
            id,
            msg_type: "call_service".to_owned(),
            domain,
            service,
            service_data,
        });
        let response = self.command(&services_req, id).await?;

        match response {
            OneShotResponse::Result(data) => {
                data.result()?;
                Ok(())
            }
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }

    /// The command subscribe_event will subscribe your client to the event bus.
    ///
    /// Returns a channel that will receive the subscription messages.
    pub async fn subscribe_event(&mut self, event_name: &str) -> HassResult<Receiver<WSEvent>> {
        let id = self.session().await?.next_seq();

        let cmd = Command::SubscribeEvent(Subscribe {
            id,
            msg_type: "subscribe_events".to_owned(),
            event_type: event_name.to_owned(),
        });

        let response = self.command(&cmd, id).await?;

        match response {
            OneShotResponse::Result(v) if v.is_ok() => {
                let rx = self.session().await?.get_rx(id);

                Ok(rx)
            }
            OneShotResponse::Result(v) => Err(HassError::ResponseError(v)),
            unknown => Err(HassError::UnknownPayloadReceived(Response::OneShot(unknown))),
        }
    }
}
