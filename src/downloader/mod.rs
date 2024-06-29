mod client;
mod download;
mod http;
mod models;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;

use models::OnErrorAction;
use thiserror::Error;
use tokio::sync::mpsc::{channel, unbounded_channel, UnboundedSender};
use tokio::sync::Notify;
use tokio::{runtime, select, task};
use tokio_util::sync::CancellationToken;

pub use self::client::Client;
use self::download::download;
use self::http::HttpClient;
pub use self::models::{DownloadRequest, OonUrl, Phase, Quality, State, StateUpdate};

#[derive(Error, Debug)]
pub enum Error {
    #[error("network error: {0}")]
    NetworkError(#[from] reqwest::Error),
    #[error("error writing to file: {0}")]
    FileError(#[from] std::io::Error),
    #[error(transparent)]
    UnexpectedError(#[from] anyhow::Error),
}

struct ClientRef {
    ctx: egui::Context,
    sender: UnboundedSender<StateUpdate>,
}

impl ClientRef {
    fn new(sender: UnboundedSender<StateUpdate>, ctx: egui::Context) -> Self {
        Self { sender, ctx }
    }

    fn send(&self, u: StateUpdate) {
        self.sender.send(u).unwrap();
        self.ctx.request_repaint();
    }
}

pub fn run(ctx: egui::Context) -> Client {
    let shutdown_token = CancellationToken::new();
    let cloned_shutdown_token = shutdown_token.clone();

    let request_queue: Arc<Mutex<VecDeque<DownloadRequest>>> =
        Arc::new(Mutex::new(VecDeque::new()));
    let request_queue_clone = request_queue.clone();
    let worker_notifier = Arc::new(Notify::new());
    let worker_notifier_clone = worker_notifier.clone();

    let (state_update_sender, state_update_receiver) = unbounded_channel::<StateUpdate>();
    let client_ref = ClientRef::new(state_update_sender, ctx);

    let (cancel_download_sender, mut cancel_download_receiver) = channel::<()>(1);
    let (on_error_sender, mut on_error_receiver) = channel::<OnErrorAction>(1);

    let thread_handle = thread::spawn(move || {
        let rt = runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .enable_io()
            .build()
            .unwrap();

        rt.block_on(async {
            let worker = task::spawn(async move {
                let http_client = HttpClient::new();
                loop {
                    let r = request_queue.lock().unwrap().pop_front();
                    if let Some(request) = r {
                        select! {
                            _ = async {
                                loop {
                                    match download(&http_client, &client_ref, request.clone()).await {
                                        Ok(()) => break,
                                        Err(e) => {
                                            log::error!("error while downloading: {}", e);
                                            client_ref.send(StateUpdate::Error(e));
                                            match on_error_receiver.recv().await.unwrap() {
                                                OnErrorAction::Retry => (),
                                                OnErrorAction::Cancel => break,
                                            }
                                        },
                                    }
                                }
                            } => {},
                            _ = cancel_download_receiver.recv() => {
                                log::info!("download cancelled");
                            }
                        }
                    } else {
                        client_ref.send(StateUpdate::Idle);
                        worker_notifier.notified().await;
                    }
                }
            });

            select! {
                _ = worker => {},
                _ = shutdown_token.cancelled() => {}
            }
        });
        log::debug!("downloader thread exited");
    });

    Client::new(
        cloned_shutdown_token,
        cancel_download_sender,
        on_error_sender,
        thread_handle,
        request_queue_clone,
        worker_notifier_clone,
        state_update_receiver,
    )
}
