use crate::config::dto::Subscriber;
use crate::config::ConfHandle;
use crate::SESSIONS_IN_PROGRESS;
use anyhow::Context as _;
use chrono::{DateTime, Utc};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use uuid::Uuid;

pub type SubscriberSender = mpsc::Sender<Message>;
pub type SubscriberReceiver = mpsc::Receiver<Message>;

pub fn subscriber_channel() -> (SubscriberSender, SubscriberReceiver) {
    mpsc::channel(64)
}

#[derive(Debug, Serialize)]
pub struct SubscriberSessionInfo {
    pub association_id: Uuid,
    pub start_timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
#[allow(clippy::enum_variant_names)]
enum MessageInner {
    #[serde(rename = "session.started")]
    SessionStarted { session: SubscriberSessionInfo },
    #[serde(rename = "session.ended")]
    SessionEnded { session: SubscriberSessionInfo },
    #[serde(rename = "session.list")]
    SessionList { session_list: Vec<SubscriberSessionInfo> },
}

#[derive(Debug, Serialize)]
pub struct Message {
    timestamp: DateTime<Utc>,
    #[serde(flatten)]
    inner: MessageInner,
}

impl Message {
    pub fn session_started(session: SubscriberSessionInfo) -> Self {
        Self {
            timestamp: session.start_timestamp,
            inner: MessageInner::SessionStarted { session },
        }
    }

    pub fn session_ended(session: SubscriberSessionInfo) -> Self {
        Self {
            timestamp: Utc::now(),
            inner: MessageInner::SessionEnded { session },
        }
    }

    pub fn session_list(session_list: Vec<SubscriberSessionInfo>) -> Self {
        Self {
            timestamp: Utc::now(),
            inner: MessageInner::SessionList { session_list },
        }
    }
}

#[instrument(skip(subscriber))]
pub async fn send_message(subscriber: &Subscriber, message: &Message) -> anyhow::Result<()> {
    use backoff::backoff::Backoff as _;
    use std::time::Duration;

    const RETRY_INITIAL_INTERVAL: Duration = Duration::from_secs(3); // initial retry interval on failure
    const RETRY_MAX_ELAPSED_TIME: Duration = Duration::from_secs(60 * 3); // retry for at most 3 minutes
    const RETRY_MULTIPLIER: f64 = 1.75; // 75% increase per back off retry

    let mut backoff = backoff::ExponentialBackoffBuilder::default()
        .with_initial_interval(RETRY_INITIAL_INTERVAL)
        .with_max_elapsed_time(Some(RETRY_MAX_ELAPSED_TIME))
        .with_multiplier(RETRY_MULTIPLIER)
        .build();

    let client = reqwest::Client::new();

    let op = || async {
        let response = client
            .post(subscriber.url.clone())
            .header("Authorization", format!("Bearer {}", subscriber.token))
            .json(message)
            .send()
            .await
            .context("Failed to post message at the subscriber URL")
            .map_err(backoff::Error::permanent)?;

        let status = response.status();

        if status.is_client_error() {
            // A client error suggest the request will never succeed no matter how many times we try
            Err(backoff::Error::permanent(anyhow::anyhow!(
                "Subscriber responded with a client error status: {status}"
            )))
        } else if status.is_server_error() {
            // However, server errors are mostly transient
            Err(backoff::Error::transient(anyhow::anyhow!(
                "Subscriber responded with a server error status: {status}"
            )))
        } else {
            Ok::<(), backoff::Error<anyhow::Error>>(())
        }
    };

    loop {
        match op().await {
            Ok(()) => break,
            Err(backoff::Error::Permanent(e)) => return Err(e),
            Err(backoff::Error::Transient { err, retry_after }) => {
                match retry_after.or_else(|| backoff.next_backoff()) {
                    Some(duration) => {
                        debug!(
                            error = format!("{err:#}"),
                            retry_after = format!("{}s", duration.as_secs()),
                            "a transient error occured"
                        );
                        tokio::time::sleep(duration).await;
                    }
                    None => return Err(err),
                }
            }
        };
    }

    trace!("message successfully sent to subscriber");

    Ok(())
}

#[instrument(skip(tx))]
pub async fn subscriber_polling_task(tx: SubscriberSender) -> anyhow::Result<()> {
    const TASK_INTERVAL: Duration = Duration::from_secs(60 * 20); // once per 20 minutes

    debug!("Task started");

    loop {
        trace!("Send session list message");

        let session_list: Vec<_> = SESSIONS_IN_PROGRESS
            .read()
            .values()
            .map(|session| SubscriberSessionInfo {
                association_id: session.association_id,
                start_timestamp: session.start_timestamp,
            })
            .collect();

        let message = Message::session_list(session_list);

        tx.send(message)
            .await
            .map_err(|e| anyhow::anyhow!("Subscriber Task ended: {e}"))?;

        sleep(TASK_INTERVAL).await;
    }
}

#[instrument(skip(conf_handle, rx))]
pub async fn subscriber_task(conf_handle: ConfHandle, mut rx: SubscriberReceiver) -> anyhow::Result<()> {
    debug!("Task started");

    let mut conf = conf_handle.get_conf();

    loop {
        tokio::select! {
            _ = conf_handle.change_notified() => {
                conf = conf_handle.get_conf();
            }
            msg = rx.recv() => {
                let msg = msg.context("All senders are dead")?;
                if let Some(subscriber) = conf.subscriber.clone() {
                    debug!(?msg, %subscriber.url, "Send message");
                    tokio::spawn(async {
                        let msg = msg;
                        let subscriber = subscriber;
                        if let Err(error) = send_message(&subscriber, &msg).await {
                            warn!(error = format!("{error:#}"), "Couldn't send message to the subscriber");
                        }
                    });
                } else {
                    trace!(?msg, "Subscriber is not configured, ignore message");
                }
            }
        }
    }
}
