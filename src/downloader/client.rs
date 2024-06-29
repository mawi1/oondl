use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tokio::sync::mpsc::{Sender, UnboundedReceiver};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::models::{OnErrorAction, QueueItem, StateUpdate};
use super::{DownloadRequest, State};

pub struct Client {
    shutdown_token: CancellationToken,
    cancel_download_sender: Sender<()>,
    on_error_sender: Sender<OnErrorAction>,
    thread_handle: Option<JoinHandle<()>>,
    request_queue: Arc<Mutex<VecDeque<DownloadRequest>>>,
    worker_notifier: Arc<Notify>,
    state_update_receiver: UnboundedReceiver<StateUpdate>,
}

impl Client {
    pub(super) fn new(
        shutdown_token: CancellationToken,
        cancel_download_sender: Sender<()>,
        on_error_sender: Sender<OnErrorAction>,
        thread_handle: JoinHandle<()>,
        request_queue: Arc<Mutex<VecDeque<DownloadRequest>>>,
        worker_notifier: Arc<Notify>,
        state_update_receiver: UnboundedReceiver<StateUpdate>,
    ) -> Self {
        Self {
            shutdown_token,
            cancel_download_sender,
            on_error_sender,
            thread_handle: Some(thread_handle),
            request_queue,
            worker_notifier,
            state_update_receiver,
        }
    }

    pub fn add_download(&mut self, request: DownloadRequest, state: &mut State) {
        let qi = QueueItem {
            request_id: request.id(),
            title: request.url.as_str().to_owned(),
        };

        let mut locked_queue = self.request_queue.lock().unwrap();
        state.enqueue(qi);
        locked_queue.push_back(request);
        drop(locked_queue);

        self.worker_notifier.notify_one();
    }

    pub fn delete_download(&mut self, id: u32) {
        self.request_queue.lock().unwrap().retain(|r| r.id() != id);
    }

    pub fn cancel_download(&self) {
        if let Err(e) = self.cancel_download_sender.blocking_send(()) {
            log::error!("could not send cancel: {}", e);
        }
    }

    pub fn retry(&self) {
        if let Err(e) = self.on_error_sender.blocking_send(OnErrorAction::Retry) {
            log::error!("could not send retry: {}", e);
        }
    }

    pub fn cancel_on_error(&self) {
        if let Err(e) = self.on_error_sender.blocking_send(OnErrorAction::Cancel) {
            log::error!("could not send cancel on error: {}", e);
        }
    }

    pub fn poll_update(&mut self) -> Option<StateUpdate> {
        self.state_update_receiver.try_recv().ok()
    }

    pub fn shutdown(&mut self) {
        self.shutdown_token.cancel();
        if let Some(handle) = self.thread_handle.take() {
            handle.join().unwrap();
        }
    }
}
