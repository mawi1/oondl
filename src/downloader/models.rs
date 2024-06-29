use std::fmt::Display;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use super::Error;

static NEXT_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Error, Debug)]
pub struct ValidationError;

impl Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "validation error")
    }
}

#[derive(Clone, Debug)]
pub struct OonUrl {
    url: Url,
    video_id: String,
    segment_id: Option<String>,
}

impl OonUrl {
    pub fn new(url_str: &str) -> Result<Self, ValidationError> {
        lazy_static! {
            static ref RE: Regex = Regex::new(
                r"^https?://on\.orf\.at/video/(?<video_id>[0-9]+)(/(?<segment_id>[0-9]+))?(/.+)?$"
            )
            .unwrap();
        }
        if let Some(cap) = RE.captures(url_str) {
            let url = Url::parse(url_str).unwrap();
            let video_id = cap.name("video_id").unwrap().as_str().to_owned();
            let segment_id = cap.name("segment_id").map(|s| s.as_str().to_owned());

            Ok(Self {
                url,
                video_id,
                segment_id,
            })
        } else {
            Err(ValidationError)
        }
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn video_id(&self) -> &str {
        &self.video_id
    }

    pub fn segment_id(&self) -> &Option<String> {
        &self.segment_id
    }
}

impl AsRef<Url> for OonUrl {
    fn as_ref(&self) -> &Url {
        &self.url
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Low,
    Medium,
    High,
}

#[derive(Clone)]
pub struct DownloadRequest {
    id: u32,
    pub url: OonUrl,
    pub quality: Quality,
    pub dest_dir: PathBuf,
}

impl DownloadRequest {
    pub fn new(url: OonUrl, quality: Quality, dest_dir: PathBuf) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            url,
            quality,
            dest_dir,
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }
}

pub(super) enum OnErrorAction {
    Retry,
    Cancel,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Phase {
    Idle,
    Analyzing,
    Downloading { video_no: (u16, u16), progress: f32 },
    Merging,
}

pub enum StateUpdate {
    StartedRequest { request_id: u32 },
    Title(String),
    StartedVideo { video_no: u16, total_videos: u16 },
    Downloaded(f32),
    Merging,
    Idle,
    Error(Error),
}

pub struct QueueItem {
    pub request_id: u32,
    pub title: String,
}

pub struct State {
    title: Option<String>,
    phase: Phase,
    queue: Vec<QueueItem>,
    error: Option<Error>,
}

impl State {
    pub fn new() -> Self {
        Self {
            title: None,
            phase: Phase::Idle,
            queue: vec![],
            error: None,
        }
    }

    pub fn update(&mut self, u: StateUpdate) {
        match u {
            StateUpdate::StartedRequest { request_id: id } => {
                self.title = None;
                self.error = None;
                self.phase = Phase::Analyzing;
                self.queue.retain(|q| q.request_id != id);
            }
            StateUpdate::Title(t) => {
                self.title = Some(t);
            }
            StateUpdate::Downloaded(p) => {
                if let Phase::Downloading {
                    ref mut progress, ..
                } = self.phase
                {
                    *progress = p;
                }
            }
            StateUpdate::Idle => {
                self.title = None;
                self.error = None;
                self.phase = Phase::Idle;
            }
            StateUpdate::Merging => {
                self.phase = Phase::Merging;
            }
            StateUpdate::StartedVideo {
                video_no,
                total_videos,
            } => {
                self.phase = Phase::Downloading {
                    video_no: (video_no, total_videos),
                    progress: 0_f32,
                }
            }
            StateUpdate::Error(e) => {
                self.error = Some(e);
            }
        }
    }

    pub(super) fn enqueue(&mut self, q: QueueItem) {
        self.queue.push(q);
    }

    pub fn retain_from_queue<F>(&mut self, f: F)
    where
        F: FnMut(&QueueItem) -> bool,
    {
        self.queue.retain(f);
    }

    pub fn queue_is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_oonurl() {
        let u1 = OonUrl::new("https://on.orf.at/video/14225330");
        assert!(u1.is_ok());
        let u2 = OonUrl::new(
            "https://on.orf.at/video/14224991/willkommen-darmstadt-mit-stermann-grissemann",
        );
        assert!(u2.is_ok());
    }

    #[test]
    fn test_invalid_oonurl() {
        let u = OonUrl::new("https://example.com/foo/a");
        assert!(u.is_err());
    }

    #[test]
    fn test_video_id() {
        let u1 = OonUrl::new("https://on.orf.at/video/14225330").unwrap();
        assert_eq!(u1.video_id(), "14225330");
        let u2 = OonUrl::new(
            "https://on.orf.at/video/14224991/willkommen-darmstadt-mit-stermann-grissemann",
        )
        .unwrap();
        assert_eq!(u2.video_id(), "14224991");
    }

    #[test]
    fn test_segment_id() {
        let u1 = OonUrl::new(
            "https://on.orf.at/video/14224991/willkommen-darmstadt-mit-stermann-grissemann",
        )
        .unwrap();
        assert_eq!(u1.segment_id(), &None);

        let u2 = OonUrl::new(
            "https://on.orf.at/video/14225651/15636092/gauder-fest-im-tiroler-zillertal",
        )
        .unwrap();
        assert_eq!(u2.segment_id(), &Some("15636092".to_owned()));
    }
}
