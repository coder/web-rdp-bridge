#![recursion_limit = "1024"]

#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate serde_derive;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use tokio::sync::RwLock;
use uuid::Uuid;

use jet_proto::token::JetSessionTokenClaims;
pub use proxy::Proxy;

use jet_proto::token::JetConnectionMode;

pub mod config;
pub mod http;
pub mod interceptor;
pub mod jet;
pub mod jet_client;
pub mod jet_rendezvous_tcp_proxy;
pub mod logger;
pub mod plugin_manager;
pub mod proxy;
pub mod rdp;
pub mod registry;
pub mod routing_client;
pub mod service;
pub mod transport;
pub mod utils;
pub mod websocket_client;

lazy_static! {
    pub static ref SESSIONS_IN_PROGRESS: RwLock<HashMap<Uuid, GatewaySessionInfo>> = RwLock::new(HashMap::new());
}

#[derive(Serialize, Clone)]
pub struct GatewaySessionInfo {
    association_id: Uuid,
    application_protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_host: Option<String>,
    connection_mode: JetConnectionMode,
    recording_policy: bool,
    filtering_policy: bool,
    start_timestamp: DateTime<Utc>,
}

impl Default for GatewaySessionInfo {
    fn default() -> Self {
        GatewaySessionInfo {
            association_id: Uuid::new_v4(),
            application_protocol: "unknown".to_string(),
            destination_host: None,
            connection_mode: JetConnectionMode::default(),
            recording_policy: false,
            filtering_policy: false,
            start_timestamp: Utc::now(),
        }
    }
}

impl GatewaySessionInfo {
    pub fn id(&self) -> Uuid {
        self.association_id
    }
}

impl From<JetSessionTokenClaims> for GatewaySessionInfo {
    fn from(session_token: JetSessionTokenClaims) -> Self {
        GatewaySessionInfo {
            association_id: session_token.jet_aid,
            application_protocol: session_token.jet_ap.clone(),
            destination_host: session_token.dst_hst.clone(),
            connection_mode: session_token.jet_cm,
            recording_policy: session_token.jet_rec,
            filtering_policy: false,
            start_timestamp: Utc::now(),
        }
    }
}

pub async fn add_session_in_progress(session: GatewaySessionInfo) {
    let mut sessions = SESSIONS_IN_PROGRESS.write().await;
    sessions.insert(session.association_id, session);
}

pub async fn remove_session_in_progress(id: Uuid) {
    SESSIONS_IN_PROGRESS.write().await.remove(&id);
}
