use core::fmt;
use std::cmp;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use camino::Utf8PathBuf;
use devolutions_gateway_task::{ShutdownSignal, Task};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, oneshot};
use tokio::{fs, io};
use typed_builder::TypedBuilder;
use uuid::Uuid;

use crate::token::{JrecTokenClaims, RecordingFileType};

const DISCONNECTED_TTL_SECS: i64 = 10;
const DISCONNECTED_TTL_DURATION: tokio::time::Duration = tokio::time::Duration::from_secs(DISCONNECTED_TTL_SECS as u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JrecFile {
    file_name: String,
    start_time: i64,
    duration: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JrecManifest {
    session_id: Uuid,
    start_time: i64,
    duration: i64,
    files: Vec<JrecFile>,
}

impl JrecManifest {
    fn read_from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let json = std::fs::read(path)?;
        let manifest = serde_json::from_slice(&json)?;
        Ok(manifest)
    }

    fn save_to_file(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&self)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

#[derive(TypedBuilder)]
pub struct ClientPush<S> {
    recordings: RecordingMessageSender,
    claims: JrecTokenClaims,
    client_stream: S,
    file_type: RecordingFileType,
    session_id: Uuid,
    shutdown_signal: ShutdownSignal,
}

impl<S> ClientPush<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub async fn run(self) -> anyhow::Result<()> {
        let Self {
            recordings,
            claims,
            mut client_stream,
            file_type,
            session_id,
            mut shutdown_signal,
        } = self;

        if session_id != claims.jet_aid {
            anyhow::bail!("inconsistent session ID (ID in token: {})", claims.jet_aid);
        }

        let recording_file = match recordings.connect(session_id, file_type).await {
            Ok(recording_file) => recording_file,
            Err(e) => {
                warn!(error = format!("{e:#}"), "Unable to start recording");
                client_stream.shutdown().await.context("shutdown")?;
                return Ok(());
            }
        };

        debug!(path = %recording_file, "Opening file");

        let res = match fs::OpenOptions::new()
            .read(false)
            .write(true)
            .truncate(true)
            .create(true)
            .open(&recording_file)
            .await
        {
            Ok(file) => {
                let mut file = BufWriter::new(file);

                let shutdown_signal = shutdown_signal.wait();
                let copy_fut = io::copy(&mut client_stream, &mut file);

                tokio::select! {
                    res = copy_fut => {
                        res.context("JREC streaming to file").map(|_| ())
                    },
                    _ = shutdown_signal => {
                        trace!("Received shutdown signal");
                        client_stream.shutdown().await.context("shutdown")
                    },
                }
            }
            Err(e) => Err(anyhow::Error::new(e).context(format!("failed to open file at {recording_file}"))),
        };

        recordings.disconnect(session_id).await.context("disconnect")?;

        res
    }
}

/// A set containing IDs of currently active recordings.
///
/// The ID is inserted at the initial recording
///
/// The purpose of this set is to provide a quick way of checking if a recording
/// is on-going for a given session ID in non-async context.
/// If you are looking for the the detailled recording state, you can use the
/// the `get_state` method provided by `RecordingMessageSender`.
#[derive(Debug)]
pub struct ActiveRecordings(Mutex<HashSet<Uuid>>);

impl ActiveRecordings {
    pub fn contains(&self, id: Uuid) -> bool {
        self.0.lock().contains(&id)
    }

    fn insert(&self, id: Uuid) -> usize {
        let mut guard = self.0.lock();
        guard.insert(id);
        guard.len()
    }

    fn remove(&self, id: Uuid) {
        self.0.lock().remove(&id);
    }
}

#[derive(Debug, Clone)]
pub enum OnGoingRecordingState {
    Connected,
    LastSeen { timestamp: i64 },
}

#[derive(Debug, Clone)]
struct OnGoingRecording {
    state: OnGoingRecordingState,
    manifest: JrecManifest,
    manifest_path: Utf8PathBuf,
}

enum RecordingManagerMessage {
    Connect {
        id: Uuid,
        file_type: RecordingFileType,
        channel: oneshot::Sender<Utf8PathBuf>,
    },
    Disconnect {
        id: Uuid,
    },
    GetState {
        id: Uuid,
        channel: oneshot::Sender<Option<OnGoingRecordingState>>,
    },
    GetCount {
        channel: oneshot::Sender<usize>,
    },
}

impl fmt::Debug for RecordingManagerMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordingManagerMessage::Connect {
                id,
                file_type,
                channel: _,
            } => f
                .debug_struct("Connect")
                .field("id", id)
                .field("file_type", file_type)
                .finish_non_exhaustive(),
            RecordingManagerMessage::Disconnect { id } => f.debug_struct("Disconnect").field("id", id).finish(),
            RecordingManagerMessage::GetState { id, channel: _ } => {
                f.debug_struct("GetState").field("id", id).finish_non_exhaustive()
            }
            RecordingManagerMessage::GetCount { channel: _ } => f.debug_struct("GetCount").finish_non_exhaustive(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecordingMessageSender {
    channel: mpsc::Sender<RecordingManagerMessage>,
    pub active_recordings: Arc<ActiveRecordings>,
}

impl RecordingMessageSender {
    async fn connect(&self, id: Uuid, file_type: RecordingFileType) -> anyhow::Result<Utf8PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.channel
            .send(RecordingManagerMessage::Connect {
                id,
                file_type,
                channel: tx,
            })
            .await
            .ok()
            .context("couldn't send New message")?;
        rx.await
            .context("couldn't receive recording file path for this recording")
    }

    async fn disconnect(&self, id: Uuid) -> anyhow::Result<()> {
        self.channel
            .send(RecordingManagerMessage::Disconnect { id })
            .await
            .ok()
            .context("couldn't send Remove message")
    }

    pub async fn get_state(&self, id: Uuid) -> anyhow::Result<Option<OnGoingRecordingState>> {
        let (tx, rx) = oneshot::channel();
        self.channel
            .send(RecordingManagerMessage::GetState { id, channel: tx })
            .await
            .ok()
            .context("couldn't send GetState message")?;
        rx.await.context("couldn't receive recording state")
    }

    pub async fn get_count(&self) -> anyhow::Result<usize> {
        let (tx, rx) = oneshot::channel();
        self.channel
            .send(RecordingManagerMessage::GetCount { channel: tx })
            .await
            .ok()
            .context("couldn't send GetCount message")?;
        rx.await.context("couldn't receive ongoing recording count")
    }
}

pub struct RecordingMessageReceiver {
    channel: mpsc::Receiver<RecordingManagerMessage>,
    active_recordings: Arc<ActiveRecordings>,
}

pub fn recording_message_channel() -> (RecordingMessageSender, RecordingMessageReceiver) {
    let ongoing_recordings = Arc::new(ActiveRecordings(Mutex::new(HashSet::new())));

    let (tx, rx) = mpsc::channel(64);

    let handle = RecordingMessageSender {
        channel: tx,
        active_recordings: ongoing_recordings.clone(),
    };

    let receiver = RecordingMessageReceiver {
        channel: rx,
        active_recordings: ongoing_recordings,
    };

    (handle, receiver)
}

struct DisconnectedTtl {
    deadline: tokio::time::Instant,
    id: Uuid,
}

impl PartialEq for DisconnectedTtl {
    fn eq(&self, other: &Self) -> bool {
        self.deadline.eq(&other.deadline) && self.id.eq(&other.id)
    }
}

impl Eq for DisconnectedTtl {}

impl PartialOrd for DisconnectedTtl {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DisconnectedTtl {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        match self.deadline.cmp(&other.deadline) {
            cmp::Ordering::Less => cmp::Ordering::Greater,
            cmp::Ordering::Equal => self.id.cmp(&other.id),
            cmp::Ordering::Greater => cmp::Ordering::Less,
        }
    }
}

pub struct RecordingManagerTask {
    rx: RecordingMessageReceiver,
    ongoing_recordings: HashMap<Uuid, OnGoingRecording>,
    recordings_path: Utf8PathBuf,
}

impl RecordingManagerTask {
    pub fn new(rx: RecordingMessageReceiver, recordings_path: Utf8PathBuf) -> Self {
        Self {
            rx,
            ongoing_recordings: HashMap::new(),
            recordings_path,
        }
    }

    async fn handle_connect(&mut self, id: Uuid, file_type: RecordingFileType) -> anyhow::Result<Utf8PathBuf> {
        const LENGTH_WARNING_THRESHOLD: usize = 1000;

        if let Some(ongoing) = self.ongoing_recordings.get(&id) {
            if matches!(ongoing.state, OnGoingRecordingState::Connected) {
                anyhow::bail!("concurrent recording for the same session is not supported");
            }
        }

        let recording_path = self.recordings_path.join(id.to_string());
        let manifest_path = recording_path.join("recording.json");

        let (manifest, recording_file) = if recording_path.exists() {
            debug!(path = %recording_path, "Recording directory already exists");

            let mut existing_manifest =
                JrecManifest::read_from_file(&manifest_path).context("read manifest from disk")?;
            let next_file_idx = existing_manifest.files.len();

            let start_time = time::OffsetDateTime::now_utc().unix_timestamp();

            let file_name = format!("recording-{next_file_idx}.{file_type}");
            let recording_file = recording_path.join(&file_name);

            existing_manifest.files.push(JrecFile {
                start_time,
                duration: 0,
                file_name,
            });

            existing_manifest
                .save_to_file(&manifest_path)
                .context("override existing manifest")?;

            (existing_manifest, recording_file)
        } else {
            debug!(path = %recording_path, "Create recording directory");

            fs::create_dir_all(&recording_path)
                .await
                .with_context(|| format!("failed to create recording path: {recording_path}"))?;

            let start_time = time::OffsetDateTime::now_utc().unix_timestamp();
            let file_name = format!("recording-0.{file_type}");
            let recording_file = recording_path.join(&file_name);

            let first_file = JrecFile {
                start_time,
                duration: 0,
                file_name,
            };

            let initial_manifest = JrecManifest {
                session_id: id,
                start_time,
                duration: 0,
                files: vec![first_file],
            };

            initial_manifest
                .save_to_file(&manifest_path)
                .context("write initial manifest to disk")?;

            (initial_manifest, recording_file)
        };

        let active_recording_count = self.rx.active_recordings.insert(id);

        self.ongoing_recordings.insert(
            id,
            OnGoingRecording {
                state: OnGoingRecordingState::Connected,
                manifest,
                manifest_path,
            },
        );
        let ongoing_recording_count = self.ongoing_recordings.len();

        // Sanity check
        if active_recording_count > LENGTH_WARNING_THRESHOLD || ongoing_recording_count > LENGTH_WARNING_THRESHOLD {
            warn!(
                active_recording_count,
                ongoing_recording_count,
                "length threshold exceeded (either the load is very high or the list is growing uncontrollably)"
            );
        }

        Ok(recording_file)
    }

    fn handle_disconnect(&mut self, id: Uuid) -> anyhow::Result<()> {
        if let Some(ongoing) = self.ongoing_recordings.get_mut(&id) {
            if !matches!(ongoing.state, OnGoingRecordingState::Connected) {
                anyhow::bail!("a recording not connected can’t be disconnected (there is probably a bug)");
            }

            let end_time = time::OffsetDateTime::now_utc().unix_timestamp();

            ongoing.state = OnGoingRecordingState::LastSeen { timestamp: end_time };

            let current_file = ongoing
                .manifest
                .files
                .last_mut()
                .context("no recording file (this is a bug)")?;
            current_file.duration = end_time - current_file.start_time;

            ongoing.manifest.duration = end_time - ongoing.manifest.start_time;

            debug!(path = %ongoing.manifest_path, "Write updated manifest to disk");

            ongoing
                .manifest
                .save_to_file(&ongoing.manifest_path)
                .with_context(|| format!("write manifest at {}", ongoing.manifest_path))?;

            Ok(())
        } else {
            Err(anyhow::anyhow!("unknown recording for ID {id}"))
        }
    }

    fn handle_remove(&mut self, id: Uuid) {
        if let Some(ongoing) = self.ongoing_recordings.get(&id) {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();

            match ongoing.state {
                // NOTE: Comparing with DISCONNECTED_TTL_SECS - 1 just in case the sleep returns faster than expected.
                // (I don’t know if this can actually happen in practice, but it’s better to be safe than sorry.)
                OnGoingRecordingState::LastSeen { timestamp } if now >= timestamp + DISCONNECTED_TTL_SECS - 1 => {
                    debug!(%id, "Mark recording as terminated");
                    self.rx.active_recordings.remove(id);
                    self.ongoing_recordings.remove(&id);

                    // TODO(DGW-86): now is a good timing to kill sessions that _must_ be recorded
                }
                _ => {
                    trace!(%id, "Recording should not be removed yet");
                }
            }
        }
    }
}

#[async_trait]
impl Task for RecordingManagerTask {
    type Output = anyhow::Result<()>;

    const NAME: &'static str = "recording manager";

    async fn run(self, shutdown_signal: ShutdownSignal) -> Self::Output {
        recording_manager_task(self, shutdown_signal).await
    }
}

#[instrument(skip_all)]
async fn recording_manager_task(
    mut manager: RecordingManagerTask,
    mut shutdown_signal: ShutdownSignal,
) -> anyhow::Result<()> {
    debug!("Task started");

    let mut disconnected = BinaryHeap::<DisconnectedTtl>::new();

    let next_remove_sleep = tokio::time::sleep_until(tokio::time::Instant::now());
    tokio::pin!(next_remove_sleep);

    // Consume initial sleep
    (&mut next_remove_sleep).await;

    loop {
        tokio::select! {
            () = &mut next_remove_sleep, if !disconnected.is_empty() => {
                // Will never panic since we check for non-emptiness before entering this block
                let to_remove = disconnected.pop().unwrap();

                manager.handle_remove(to_remove.id);

                // Re-arm the Sleep instance with the next deadline if required
                if let Some(next) = disconnected.peek() {
                    next_remove_sleep.as_mut().reset(next.deadline)
                }
            }
            msg = manager.rx.channel.recv() => {
                let Some(msg) = msg else {
                    warn!("All senders are dead");
                    break;
                };

                debug!(?msg, "Received message");

                match msg {
                    RecordingManagerMessage::Connect { id, file_type, channel  } => {
                        match manager.handle_connect(id, file_type).await {
                            Ok(recording_file) => {
                                let _ = channel.send(recording_file);
                            }
                            Err(e) => error!(error = format!("{e:#}"), "handle_connect"),
                        }
                    },
                    RecordingManagerMessage::Disconnect { id } => {
                        if let Err(e) = manager.handle_disconnect(id) {
                            error!(error = format!("{e:#}"), "handle_disconnect");
                        }

                        let now = tokio::time::Instant::now();
                        let deadline = now + DISCONNECTED_TTL_DURATION;

                        disconnected.push(DisconnectedTtl {
                            deadline,
                            id,
                        });

                        // Reset the Sleep instance if the new deadline is sooner or it is already elapsed
                        if next_remove_sleep.is_elapsed() || deadline < next_remove_sleep.deadline() {
                            next_remove_sleep.as_mut().reset(deadline);
                        }
                    }
                    RecordingManagerMessage::GetState { id, channel } => {
                        let response = manager.ongoing_recordings.get(&id).map(|ongoing| ongoing.state.clone());
                        let _ = channel.send(response);
                    }
                    RecordingManagerMessage::GetCount { channel } => {
                        let _ = channel.send(manager.ongoing_recordings.len());
                    }
                }
            }
            _ = shutdown_signal.wait() => {
                break;
            }
        }
    }

    debug!("Task is stopping; wait for disconnect messages");

    while let Some(msg) = manager.rx.channel.recv().await {
        debug!(?msg, "Received message");
        if let RecordingManagerMessage::Disconnect { id } = msg {
            if let Err(e) = manager.handle_disconnect(id) {
                error!(error = format!("{e:#}"), "handle_disconnect");
            }
            manager.ongoing_recordings.remove(&id);
        }
    }

    debug!("Task terminated");

    Ok(())
}
