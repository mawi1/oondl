mod extract;
mod mpd;

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::{fs, try_join};
use url::Url;

use self::extract::VideoInfo::*;
use self::extract::{extract_segment_url, extract_title, extract_video_info};
use self::mpd::MediaUrls;
use super::http::{HttpClient, Response};
use super::{ClientRef, DownloadRequest, Error, Quality, StateUpdate};

async fn check_mp4_path(dir: &Path, file_stem: &str) -> Result<PathBuf, io::Error> {
    let mut file_suffix = None;
    let mut suffix_no = 1_u8;

    loop {
        let file_path = dir.join(format!(
            "{}{}.mp4",
            file_stem,
            file_suffix.as_deref().unwrap_or_default()
        ));
        if fs::try_exists(&file_path).await? {
            if suffix_no == u8::MAX {
                break Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            } else {
                file_suffix = Some(format!("_({})", suffix_no));
                suffix_no += 1;
            }
        } else {
            break Ok(file_path);
        }
    }
}

async fn run_ffmpeg<I, S>(args: I, opt_current_dir: Option<&Path>) -> anyhow::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut c = Command::new("ffmpeg");
    c.stdin(Stdio::null());
    c.args(args);
    if let Some(current_dir) = opt_current_dir {
        c.current_dir(current_dir);
    }

    let output = c.output().await.context("failed to run ffmpeg")?;
    log::debug!(
        "stdout of ffmpeg: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    log::debug!(
        "stderr of ffmpeg: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("ffmpeg exited with non-zero exit code");
    }

    Ok(())
}

async fn download_video(
    http_client: &HttpClient,
    client_ref: &ClientRef,
    mpd_url: Url,
    quality: Quality,
    dest_dir: &Path,
    dest_path: &Path,
) -> Result<(), Error> {
    let temp_dir: TempDir = TempDir::new_in(dest_dir)?;

    let Response {
        body: mpd_xml,
        final_url,
    } = http_client.get(mpd_url).await?;
    let MediaUrls { video, audio } = mpd::get_urls(&final_url, &mpd_xml, quality)?;

    let total_chunks = (video.len() + audio.len()) as f32;
    let mut chunks_downloaded = 0_f32;
    let mut last_progress = 0_f32;

    let handle_chunk_downloaded = Arc::new(Mutex::new(|| {
        chunks_downloaded += 1_f32;
        let progress = chunks_downloaded / total_chunks;
        if progress - last_progress > 0.01 || progress == 1_f32 {
            client_ref.send(StateUpdate::Downloaded(progress));
            last_progress = progress;
            log::debug!("progress: {}", progress);
        }
    }));
    let handle_chunk_downloaded_clone = handle_chunk_downloaded.clone();

    let video_path = temp_dir.path().join("video.mp4");
    let dl_video = http_client.download_to_file(&video_path, video, handle_chunk_downloaded);
    let audio_path = temp_dir.path().join("audio.mp4");
    let dl_audio = http_client.download_to_file(&audio_path, audio, handle_chunk_downloaded_clone);
    try_join!(dl_video, dl_audio)?;

    client_ref.send(StateUpdate::Merging);
    run_ffmpeg(
        &[
            OsStr::new("-i"),
            video_path.as_os_str(),
            OsStr::new("-i"),
            audio_path.as_os_str(),
            OsStr::new("-codec"),
            OsStr::new("copy"),
            OsStr::new("-map"),
            OsStr::new("0:v"),
            OsStr::new("-map"),
            OsStr::new("1:a"),
            dest_path.as_os_str(),
        ],
        None,
    )
    .await?;

    Ok(())
}

pub(super) async fn download(
    http_client: &HttpClient,
    client_ref: &ClientRef,
    request: DownloadRequest,
) -> Result<(), Error> {
    client_ref.send(StateUpdate::StartedRequest {
        request_id: request.id(),
    });

    let id = request.url.video_id().to_owned();
    let Response { body: html, .. } = http_client.get(request.url.as_ref().clone()).await?;
    let title = extract_title(&html)?;

    client_ref.send(StateUpdate::Title(title.clone()));

    let mut dest_name = title
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect::<String>();
    dest_name = sanitise_file_name::sanitise(&dest_name);
    dest_name.push_str("_");
    dest_name.push_str(&id);

    if let Some(segment_id) = request.url.segment_id() {
        let url = extract_segment_url(&html, segment_id)?;
        let dest_path = check_mp4_path(&request.dest_dir, &dest_name).await?;
        client_ref.send(StateUpdate::StartedVideo {
            video_no: 1,
            total_videos: 1,
        });
        download_video(http_client, client_ref, url, request.quality, &request.dest_dir, &dest_path).await?;
    } else {
        let dest_path = check_mp4_path(&request.dest_dir, &dest_name).await?;
        match extract_video_info(&html)? {
            Unsegmented(mpd_url) => {
                client_ref.send(StateUpdate::StartedVideo {
                    video_no: 1,
                    total_videos: 1,
                });
                download_video(http_client, client_ref, mpd_url, request.quality, &request.dest_dir, &dest_path)
                    .await?;
            }
            Segmented(mpd_urls) => {
                let temp_dir = TempDir::new_in(&request.dest_dir)?;
                let total_videos = mpd_urls.len() as u16;
                let mut concat_list = String::new();

                for (idx, mpd_url) in mpd_urls.into_iter().enumerate() {
                    let file_name = format!("{}.mp4", idx);
                    let seg_dest_path = temp_dir.path().join(&file_name);
                    client_ref.send(StateUpdate::StartedVideo {
                        video_no: idx as u16 + 1,
                        total_videos,
                    });
                    download_video(
                        http_client,
                        client_ref,
                        mpd_url,
                        request.quality,
                        temp_dir.path(),
                        &seg_dest_path,
                    )
                    .await?;
                    concat_list.push_str(&format!("file '{}'\n", &file_name));
                }

                fs::write(temp_dir.path().join("concat.txt"), concat_list).await?;
                client_ref.send(StateUpdate::Merging);
                run_ffmpeg(
                    &[
                        OsStr::new("-f"),
                        OsStr::new("concat"),
                        OsStr::new("-i"),
                        OsStr::new("concat.txt"),
                        OsStr::new("-codec"),
                        OsStr::new("copy"),
                        dest_path.as_os_str(),
                    ],
                    Some(temp_dir.path()),
                )
                .await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn test_check_mp4_path() {
        let temp_dir = TempDir::new().unwrap();

        let p = check_mp4_path(temp_dir.path(), "foo").await.unwrap();
        assert_eq!(p.file_name().unwrap(), "foo.mp4");
    }

    #[tokio::test]
    async fn test_check_mp4_path_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        File::create_new(temp_dir.path().join("foo.mp4")).unwrap();

        let p = check_mp4_path(temp_dir.path(), "foo").await.unwrap();
        assert_eq!(p.file_name().unwrap(), "foo_(1).mp4");
    }

    #[tokio::test]
    async fn test_check_mp4_path_2_files_exist() {
        let temp_dir = TempDir::new().unwrap();
        File::create_new(temp_dir.path().join("foo.mp4")).unwrap();
        File::create_new(temp_dir.path().join("foo_(1).mp4")).unwrap();

        let p = check_mp4_path(temp_dir.path(), "foo").await.unwrap();
        assert_eq!(p.file_name().unwrap(), "foo_(2).mp4");
    }

    #[tokio::test]
    async fn test_check_mp4_path_256_files_exist() {
        let temp_dir = TempDir::new().unwrap();
        File::create_new(temp_dir.path().join("foo.mp4")).unwrap();
        for n in 1..=255 {
            File::create_new(temp_dir.path().join(format!("foo_({}).mp4", n))).unwrap();
        }

        let p_res = check_mp4_path(temp_dir.path(), "foo");
        assert_eq!(
            p_res.await.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
    }
}
